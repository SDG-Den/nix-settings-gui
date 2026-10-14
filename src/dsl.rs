use rnix::ast::{Attr, Expr, HasEntry, InterpolPart};
use rowan::ast::AstNode;
use serde::{Deserialize, Serialize};

/// A literal value usable inside a marker body. Bodies use real Nix literals,
/// so only literal expressions are representable here.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Lit>),
    Null,
}

/// Parsed `key = value;` pairs from a single marker body.
#[derive(Debug, Clone, Default)]
pub struct Directives(Vec<(String, Lit)>);

impl Directives {
    /// Wrap the body in `{ ... }` and parse it with rnix, then read every
    /// literal binding into a key/value list.
    pub fn parse(body: &str) -> Result<Self, String> {
        let src = format!("{{\n{body}\n}}");
        let root = rnix::Root::parse(&src)
            .ok()
            .map_err(|e| format!("invalid marker body: {e}"))?;
        let expr = root.expr().ok_or("empty marker body")?;
        let Expr::AttrSet(set) = expr else {
            return Err("marker body must be an attribute set".into());
        };
        let mut pairs = Vec::new();
        for binding in set.attrpath_values() {
            let Some(path) = binding.attrpath() else {
                continue;
            };
            let mut key = Vec::new();
            for attr in path.attrs() {
                match attr_name(&attr) {
                    Some(name) => key.push(name),
                    None => break,
                }
            }
            let Some(value) = binding.value() else {
                continue;
            };
            if let Some(lit) = eval_expr(&value) {
                pairs.push((key.join("."), lit));
            }
        }
        Ok(Directives(pairs))
    }

    pub fn get(&self, key: &str) -> Option<&Lit> {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Iterate the raw `key = value` pairs in source order.
    pub fn iter(&self) -> impl Iterator<Item = &(String, Lit)> {
        self.0.iter()
    }

    /// A view of every key under `prefix.`, with the prefix stripped. Keys at
    /// the top level that do not share the prefix are dropped.
    pub fn sub(&self, prefix: &str) -> Directives {
        let prefix = format!("{prefix}.");
        Directives(
            self.0
                .iter()
                .filter_map(|(k, v)| {
                    k.strip_prefix(&prefix)
                        .map(|rest| (rest.to_string(), v.clone()))
                })
                .collect(),
        )
    }

    pub fn string(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Lit::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            Lit::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            Lit::Int(i) => Some(*i as f64),
            Lit::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            Lit::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn strings(&self, key: &str) -> Option<Vec<String>> {
        match self.get(key)? {
            Lit::List(items) => Some(
                items
                    .iter()
                    .filter_map(|item| match item {
                        Lit::Str(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        }
    }
}

fn eval_expr(expr: &Expr) -> Option<Lit> {
    match expr {
        Expr::Str(s) => string_value(s).map(Lit::Str),
        Expr::Literal(lit) => match lit.kind() {
            rnix::ast::LiteralKind::Integer(i) => i.value().ok().map(Lit::Int),
            rnix::ast::LiteralKind::Float(f) => f.value().ok().map(Lit::Float),
            rnix::ast::LiteralKind::Uri(_) => None,
        },
        Expr::Ident(id) => match id.syntax().text().to_string().as_str() {
            "true" => Some(Lit::Bool(true)),
            "false" => Some(Lit::Bool(false)),
            "null" => Some(Lit::Null),
            _ => None,
        },
        Expr::UnaryOp(op) => match eval_expr(&op.expr()?)? {
            Lit::Int(i) => Some(Lit::Int(-i)),
            Lit::Float(f) => Some(Lit::Float(-f)),
            _ => None,
        },
        Expr::Paren(p) => eval_expr(&p.expr()?),
        Expr::List(list) => {
            let mut items = Vec::new();
            for item in list.items() {
                items.push(eval_expr(&item)?);
            }
            Some(Lit::List(items))
        }
        _ => None,
    }
}

fn string_value(s: &rnix::ast::Str) -> Option<String> {
    match s.normalized_parts().as_slice() {
        [InterpolPart::Literal(text)] => Some(text.clone()),
        _ => None,
    }
}

fn attr_name(attr: &Attr) -> Option<String> {
    match attr {
        Attr::Ident(ident) => Some(ident.syntax().text().to_string()),
        Attr::Str(s) => Some(string_value(s)?),
        Attr::Dynamic(_) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    Menu,
    Option,
    Template,
}

/// Recognise a marker comment and parse its body. `inner` is the comment text
/// with the `/*` and `*/` delimiters already stripped.
pub fn parse_marker(inner: &str) -> Option<(MarkerKind, Directives)> {
    let (kind, rest) = split_marker(inner.trim_start())?;
    let directives = Directives::parse(rest).ok()?;
    Some((kind, directives))
}

/// Strip comment delimiters and hand the inside to `parse_marker`. Both
/// `/* ... */` block comments and `# ...` line comments are accepted.
pub fn parse_comment(text: &str) -> Option<(MarkerKind, Directives)> {
    let trimmed = text.trim();
    let inner = if let Some(rest) = trimmed.strip_prefix("/*") {
        rest.strip_suffix("*/")?
    } else {
        trimmed.strip_prefix('#')?
    };
    parse_marker(inner)
}

fn split_marker(s: &str) -> Option<(MarkerKind, &str)> {
    for (tag, kind) in [
        ("@menu", MarkerKind::Menu),
        ("@option", MarkerKind::Option),
        ("@template", MarkerKind::Template),
    ] {
        if let Some(rest) = s.strip_prefix(tag) {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return Some((kind, rest));
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetKind {
    Switch,
    Dropdown,
    Radio,
    Spin,
    Slider,
    Text,
    Multiline,
    Path,
    Package,
    Packages,
    List,
    NixosModules,
    HmModules,
    Color,
    Submodule,
    Raw,
}

impl WidgetKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "switch" => Self::Switch,
            "dropdown" => Self::Dropdown,
            "radio" => Self::Radio,
            "spin" => Self::Spin,
            "slider" => Self::Slider,
            "text" => Self::Text,
            "multiline" => Self::Multiline,
            "path" => Self::Path,
            "package" => Self::Package,
            "packages" => Self::Packages,
            "list" => Self::List,
            "nixosmodules" | "nixos-modules" => Self::NixosModules,
            "hmmodules" | "hm-modules" | "homemanagermodules" => Self::HmModules,
            "color" => Self::Color,
            "submodule" => Self::Submodule,
            "raw" => Self::Raw,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MenuSpec {
    pub displayname: Option<String>,
    pub icon: Option<String>,
    pub order: Option<i64>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub systems: Vec<String>,
    pub hidden: bool,
}

impl MenuSpec {
    pub fn from_directives(d: &Directives) -> Self {
        Self {
            displayname: d.string("displayname"),
            icon: d.string("icon"),
            order: d.int("order"),
            description: d.string("description"),
            category: d.string("category"),
            systems: d.strings("systems").unwrap_or_default(),
            hidden: d.bool("hidden").unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OptionSpec {
    pub widget: Option<WidgetKind>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub choices: Option<Vec<String>>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub digits: Option<u32>,
    pub unit: Option<String>,
    pub rows: Option<u32>,
    pub placeholder: Option<String>,
    pub group: Option<String>,
    pub order: Option<i64>,
    pub advanced: bool,
    pub hidden: bool,
    pub readonly: bool,
    pub nullable: bool,
    pub pattern: Option<String>,
    pub item_label: Option<String>,
    pub add_label: Option<String>,
}

impl OptionSpec {
    pub fn from_directives(d: &Directives) -> Self {
        Self {
            widget: d.string("widget").and_then(|w| WidgetKind::parse(&w)),
            label: d.string("label"),
            description: d.string("description"),
            choices: d.strings("choices"),
            min: d.number("min"),
            max: d.number("max"),
            step: d.number("step"),
            digits: d.int("digits").map(|n| n.max(0) as u32),
            unit: d.string("unit"),
            rows: d.int("rows").map(|n| n.max(0) as u32),
            placeholder: d.string("placeholder"),
            group: d.string("group"),
            order: d.int("order"),
            advanced: d.bool("advanced").unwrap_or(false),
            hidden: d.bool("hidden").unwrap_or(false),
            readonly: d.bool("readonly").unwrap_or(false),
            nullable: d.bool("nullable").unwrap_or(false),
            pattern: d.string("pattern"),
            item_label: d.string("itemLabel"),
            add_label: d.string("addLabel"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_menu_marker() {
        let text = "/* @menu\n displayname = \"Audio\";\n icon = \"audio-card-symbolic\";\n order = 40;\n systems = [ \"x570\" \"webhost\" ];\n hidden = false;\n*/";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Menu);
        let menu = MenuSpec::from_directives(&d);
        assert_eq!(menu.displayname.as_deref(), Some("Audio"));
        assert_eq!(menu.icon.as_deref(), Some("audio-card-symbolic"));
        assert_eq!(menu.order, Some(40));
        assert_eq!(menu.systems, vec!["x570", "webhost"]);
        assert!(!menu.hidden);
    }

    #[test]
    fn parses_template_marker_and_sub() {
        let text = "/* @template\n label = \"Example\";\n kind = \"nixos\";\n placeholders.name.label = \"Module name\";\n placeholders.name.default = \"example\";\n*/";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Template);
        assert_eq!(d.string("label").as_deref(), Some("Example"));
        let placeholders = d.sub("placeholders");
        assert_eq!(
            placeholders.string("name.label").as_deref(),
            Some("Module name")
        );
        assert_eq!(
            placeholders.string("name.default").as_deref(),
            Some("example")
        );
    }

    #[test]
    fn parses_option_marker() {
        let text = "/* @option widget = \"dropdown\"; label = \"Time zone\"; choices = [ \"Europe/Amsterdam\" \"UTC\" ]; */";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Option);
        let opt = OptionSpec::from_directives(&d);
        assert_eq!(opt.widget, Some(WidgetKind::Dropdown));
        assert_eq!(opt.label.as_deref(), Some("Time zone"));
        assert_eq!(
            opt.choices,
            Some(vec!["Europe/Amsterdam".into(), "UTC".into()])
        );
    }

    #[test]
    fn parses_numeric_and_bool_keys() {
        let d =
            Directives::parse("min = -1.5; max = 100; step = 0.5; digits = 2; advanced = true;")
                .unwrap();
        let opt = OptionSpec::from_directives(&d);
        assert_eq!(opt.min, Some(-1.5));
        assert_eq!(opt.max, Some(100.0));
        assert_eq!(opt.step, Some(0.5));
        assert_eq!(opt.digits, Some(2));
        assert!(opt.advanced);
    }

    #[test]
    fn parses_hash_line_comments() {
        let (kind, d) =
            parse_comment("# @option widget = \"packages\"; label = \"Base\";").unwrap();
        assert_eq!(kind, MarkerKind::Option);
        let opt = OptionSpec::from_directives(&d);
        assert_eq!(opt.widget, Some(WidgetKind::Packages));
        assert_eq!(opt.label.as_deref(), Some("Base"));
        assert!(parse_comment("# just a note").is_none());
    }

    #[test]
    fn ignores_plain_comments() {
        assert!(parse_comment("/* just a note */").is_none());
        assert!(parse_marker("# not a marker").is_none());
        assert!(parse_marker("@menux = 1;").is_none());
    }

    #[test]
    fn marker_requires_word_boundary() {
        assert!(parse_marker("@optionwidget = 1;").is_none());
        assert!(parse_marker("@option").is_some());
    }
}
