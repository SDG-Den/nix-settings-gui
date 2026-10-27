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

impl Lit {
    /// Render a literal as canonical Nix source, so it can be compared against
    /// the output of an editor's `to_nix`.
    pub fn to_nix(&self) -> String {
        match self {
            Lit::Bool(b) => b.to_string(),
            Lit::Int(i) => i.to_string(),
            Lit::Float(f) => format_float(*f),
            Lit::Str(s) => quote(s),
            Lit::List(items) => {
                let inner: Vec<String> = items.iter().map(Lit::to_nix).collect();
                format!("[ {} ]", inner.join(" "))
            }
            Lit::Null => "null".to_string(),
        }
    }
}

fn quote(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${");
    format!("\"{escaped}\"")
}

fn format_float(f: f64) -> String {
    let mut text = f.to_string();
    if !text.contains('.') && !text.contains('e') && !text.contains('E') {
        text.push_str(".0");
    }
    text
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
    Snippet,
    Insert,
    TableItem,
    Inputs,
    WidgetPreset,
    WidgetPresetField,
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
        ("@snippet", MarkerKind::Snippet),
        ("@insert", MarkerKind::Insert),
        ("@table-item", MarkerKind::TableItem),
        ("@inputs", MarkerKind::Inputs),
        ("@widget-preset", MarkerKind::WidgetPreset),
        ("@widget-preset-field", MarkerKind::WidgetPresetField),
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
    HorizontalSubmodule,
    GridConfiguration,
    TableAttrset,
    TableList,
    Keybinds,
    Monitors,
    Theme,
    Wallpaper,
    Menu,
    Plugins,
    Bar,
    Dock,
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
            "horizontal-submodule" | "horizontalsubmodule" => Self::HorizontalSubmodule,
            "grid-configuration" | "gridconfiguration" => Self::GridConfiguration,
            "table-attrset" | "tableattrset" => Self::TableAttrset,
            "table-list" | "tablelist" => Self::TableList,
            "keybinds" => Self::Keybinds,
            "monitors" => Self::Monitors,
            "theme" => Self::Theme,
            "wallpaper" => Self::Wallpaper,
            "menu" => Self::Menu,
            "plugins" => Self::Plugins,
            "bar" => Self::Bar,
            "dock" => Self::Dock,
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
pub struct Visibility {
    /// Full option path of the governing option, within the same menu.
    pub path: String,
    /// Canonical Nix literal the governing value must equal, if any.
    pub equals: Option<String>,
    /// Canonical Nix literal the governing value must not equal, if any.
    pub not_equals: Option<String>,
}

impl Visibility {
    /// Whether the condition holds for the governing option's current value.
    /// `None` means the governing option was not found, which fails open.
    pub fn holds(&self, current: Option<&str>) -> bool {
        let Some(current) = current else {
            return true;
        };
        let current = current.trim();
        if self.equals.is_none() && self.not_equals.is_none() {
            return !matches!(current, "false" | "null" | "0" | "0.0");
        }
        if let Some(equals) = &self.equals {
            if current != equals.trim() {
                return false;
            }
        }
        if let Some(not_equals) = &self.not_equals {
            if current == not_equals.trim() {
                return false;
            }
        }
        true
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
    pub width_chars: Option<u32>,
    pub group: Option<String>,
    pub group_order: Option<i64>,
    pub order: Option<i64>,
    pub max_columns: Option<u32>,
    pub grid_columns: Option<i64>,
    pub columns: Option<Vec<String>>,
    pub column_types: Option<Vec<String>>,
    pub horizontal: bool,
    pub key_editable: bool,
    pub first_value_editable: bool,
    pub advanced: bool,
    pub hidden: bool,
    pub readonly: bool,
    pub nullable: bool,
    pub pattern: Option<String>,
    pub item_label: Option<String>,
    pub add_label: Option<String>,
    pub visible_when: Option<Visibility>,
    /// Directory holding wallpaper images, relative to the file containing the
    /// option. Defaults to `./wallpapers`.
    pub wallpaper_dir: Option<String>,
    /// Flake input that declares the optional wallpaper presets, e.g.
    /// `ojuny-desktop`. When set, the wallpaper picker lists those presets in
    /// addition to the images in `wallpaperDir`.
    pub preset_source: Option<String>,
    /// Directory holding icons for `icon` table columns, relative to the file
    /// containing the option. Defaults to `./icons`.
    pub icon_dir: Option<String>,
    /// Shell command that applies the option immediately, e.g. an IPC call.
    /// `%%val%%` in the command stands for the option's new value.
    pub apply_command: Option<String>,
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
            width_chars: d.int("width").map(|n| n.clamp(1, i32::MAX as i64) as u32),
            group: d.string("group"),
            group_order: d.int("groupOrder"),
            order: d.int("order"),
            max_columns: d.int("maxColumns").map(|n| n.max(1) as u32),
            grid_columns: d.int("gridColumns"),
            columns: d.strings("columns"),
            column_types: d.strings("columnTypes"),
            horizontal: d.bool("horizontal").unwrap_or(false),
            key_editable: d.bool("keyEditable").unwrap_or(false),
            first_value_editable: d.bool("firstValueEditable").unwrap_or(false),
            advanced: d.bool("advanced").unwrap_or(false),
            hidden: d.bool("hidden").unwrap_or(false),
            readonly: d.bool("readonly").unwrap_or(false),
            nullable: d.bool("nullable").unwrap_or(false),
            pattern: d.string("pattern"),
            item_label: d.string("itemLabel"),
            add_label: d.string("addLabel"),
            visible_when: parse_visibility(d),
            wallpaper_dir: d.string("wallpaperDir"),
            preset_source: d.string("presetSource"),
            icon_dir: d.string("iconDir"),
            apply_command: d.string("applyCommand"),
        }
    }
}

/// Build a `Visibility` from the `visibleWhen.*` dotted directives, if a path
/// is present.
fn parse_visibility(d: &Directives) -> Option<Visibility> {
    let path = d.string("visibleWhen.path")?;
    Some(Visibility {
        path,
        equals: d.get("visibleWhen.equals").map(|lit| lit.to_nix()),
        not_equals: d.get("visibleWhen.notEquals").map(|lit| lit.to_nix()),
    })
}

/// One `%%token%%` field offered when a snippet is inserted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaceholderSpec {
    pub name: String,
    pub label: String,
    pub default: String,
    pub hint: String,
}

/// A reusable snippet defined by an in-file `@snippet` marker.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnippetSpec {
    pub name: String,
    pub label: String,
    pub body: String,
    pub placeholders: Vec<PlaceholderSpec>,
}

impl SnippetSpec {
    pub fn from_directives(d: &Directives) -> Self {
        let name = d.string("name").unwrap_or_default();
        let body = d
            .string("snippet")
            .or_else(|| d.string("body"))
            .unwrap_or_default();
        let label = d.string("label").unwrap_or_else(|| name.clone());
        Self {
            name,
            label,
            placeholders: collect_placeholders(d, &body),
            body,
        }
    }
}

/// A `@insert` marker naming the snippet to place there.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InsertSpec {
    pub name: String,
    pub group: Option<String>,
    pub order: Option<i64>,
    pub label: Option<String>,
}

impl InsertSpec {
    pub fn from_directives(d: &Directives) -> Self {
        Self {
            name: d.string("name").unwrap_or_default(),
            group: d.string("group"),
            order: d.int("order"),
            label: d.string("label"),
        }
    }
}

/// The `%%token%%` names in `body`, in first-seen order, deduplicated.
pub fn snippet_tokens(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("%%") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("%%") else {
            break;
        };
        let name = &after[..end];
        if !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        rest = &after[end + 2..];
    }
    names
}

/// Read `placeholders.<name>.<field>` overrides for the tokens in `body`,
/// preserving the token order from the body.
fn collect_placeholders(d: &Directives, body: &str) -> Vec<PlaceholderSpec> {
    let sub = d.sub("placeholders");
    snippet_tokens(body)
        .into_iter()
        .map(|name| PlaceholderSpec {
            label: sub
                .string(&format!("{name}.label"))
                .unwrap_or_else(|| name.clone()),
            default: sub.string(&format!("{name}.default")).unwrap_or_default(),
            hint: sub.string(&format!("{name}.hint")).unwrap_or_default(),
            name,
        })
        .collect()
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
    fn parses_wallpaper_dir() {
        let text = "/* @option widget = \"wallpaper\"; wallpaperDir = \"./wallpapers\"; */";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Option);
        let opt = OptionSpec::from_directives(&d);
        assert_eq!(opt.widget, Some(WidgetKind::Wallpaper));
        assert_eq!(opt.wallpaper_dir.as_deref(), Some("./wallpapers"));
    }

    #[test]
    fn parses_preset_source() {
        let text = "/* @option widget = \"wallpaper\"; presetSource = \"ojuny-desktop\"; */";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Option);
        let opt = OptionSpec::from_directives(&d);
        assert_eq!(opt.preset_source.as_deref(), Some("ojuny-desktop"));
    }

    #[test]
    fn parses_icon_dir() {
        let text =
            "/* @option widget = \"table-attrset\"; columnTypes = [ \"icon\" ]; iconDir = \"./icons\"; */";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Option);
        let opt = OptionSpec::from_directives(&d);
        assert_eq!(opt.icon_dir.as_deref(), Some("./icons"));
    }

    #[test]
    fn parses_apply_command() {
        let text =
            "/* @option widget = \"switch\"; applyCommand = \"dms ipc call theme set %%val%%\"; */";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Option);
        let opt = OptionSpec::from_directives(&d);
        assert_eq!(
            opt.apply_command.as_deref(),
            Some("dms ipc call theme set %%val%%")
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

    #[test]
    fn parses_visibility_dependency() {
        let text = "/* @option widget = \"switch\"; visibleWhen.path = \"services.pipewire.enable\"; visibleWhen.equals = true; visibleWhen.notEquals = false; */";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Option);
        let opt = OptionSpec::from_directives(&d);
        let vis = opt.visible_when.unwrap();
        assert_eq!(vis.path, "services.pipewire.enable");
        assert_eq!(vis.equals.as_deref(), Some("true"));
        assert_eq!(vis.not_equals.as_deref(), Some("false"));
    }

    #[test]
    fn visibility_holds_by_truthiness_and_equality() {
        let truthy = Visibility {
            path: "a".into(),
            equals: None,
            not_equals: None,
        };
        assert!(truthy.holds(Some("true")));
        assert!(!truthy.holds(Some("false")));
        assert!(truthy.holds(None));

        let eq = Visibility {
            path: "a".into(),
            equals: Some("\"auto\"".into()),
            not_equals: None,
        };
        assert!(eq.holds(Some("\"auto\"")));
        assert!(!eq.holds(Some("\"manual\"")));

        let ne = Visibility {
            path: "a".into(),
            equals: None,
            not_equals: Some("false".into()),
        };
        assert!(ne.holds(Some("true")));
        assert!(!ne.holds(Some("false")));
    }

    #[test]
    fn visibility_equality_matches_editor_literals() {
        let bool_true = Visibility {
            path: "a".into(),
            equals: Some("true".into()),
            not_equals: None,
        };
        assert!(bool_true.holds(Some("true")));
        assert!(!bool_true.holds(Some("false")));

        let bool_false = Visibility {
            path: "a".into(),
            equals: Some("false".into()),
            not_equals: None,
        };
        assert!(bool_false.holds(Some("false")));
        assert!(!bool_false.holds(Some("true")));

        let string_eq = Visibility {
            path: "a".into(),
            equals: Some("\"firefox\"".into()),
            not_equals: None,
        };
        assert!(string_eq.holds(Some("\"firefox\"")));
        assert!(!string_eq.holds(Some("\"chrome\"")));
    }

    #[test]
    fn lit_renders_canonical_nix() {
        assert_eq!(Lit::Bool(true).to_nix(), "true");
        assert_eq!(Lit::Int(3).to_nix(), "3");
        assert_eq!(Lit::Str("a\"b".into()).to_nix(), "\"a\\\"b\"");
        assert_eq!(
            Lit::List(vec![Lit::Int(1), Lit::Bool(false)]).to_nix(),
            "[ 1 false ]"
        );
    }

    #[test]
    fn parses_snippet_marker() {
        let text = "/* @snippet\n   name = \"keybind\";\n   label = \"Add keybind\";\n   snippet = ''\n     # @option widget = \"text\";\n     keybinds.\"%%keys%%\" = \"%%action%%\";\n   '';\n   placeholders.keys.label = \"Keys\";\n*/";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Snippet);
        let spec = SnippetSpec::from_directives(&d);
        assert_eq!(spec.name, "keybind");
        assert_eq!(spec.label, "Add keybind");
        assert!(spec.body.contains("%%keys%%"));
        assert!(spec.body.contains("# @option"));
        let names: Vec<&str> = spec.placeholders.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["keys", "action"]);
        assert_eq!(spec.placeholders[0].label, "Keys");
        assert_eq!(spec.placeholders[1].label, "action");
    }

    #[test]
    fn parses_insert_marker() {
        let text = "/* @insert name = \"keybind\"; group = \"Keybinds\"; order = 10; */";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Insert);
        let spec = InsertSpec::from_directives(&d);
        assert_eq!(spec.name, "keybind");
        assert_eq!(spec.group.as_deref(), Some("Keybinds"));
        assert_eq!(spec.order, Some(10));
    }

    #[test]
    fn parses_table_and_bespoke_widget_directives() {
        let text = "/* @option widget = \"table-attrset\"; columns = [ \"enable\" \"url\" ]; columnTypes = [ \"bool\" \"str\" ]; keyEditable = true; firstValueEditable = true; maxColumns = 3; groupOrder = 20; */";
        let (kind, d) = parse_comment(text).unwrap();
        assert_eq!(kind, MarkerKind::Option);
        let opt = OptionSpec::from_directives(&d);
        assert_eq!(opt.widget, Some(WidgetKind::TableAttrset));
        assert_eq!(opt.columns, Some(vec!["enable".into(), "url".into()]));
        assert_eq!(opt.column_types, Some(vec!["bool".into(), "str".into()]));
        assert!(opt.key_editable);
        assert!(opt.first_value_editable);
        assert_eq!(opt.max_columns, Some(3));
        assert_eq!(opt.group_order, Some(20));
    }

    #[test]
    fn parses_bespoke_widget_names() {
        assert_eq!(WidgetKind::parse("table-list"), Some(WidgetKind::TableList));
        assert_eq!(
            WidgetKind::parse("horizontal-submodule"),
            Some(WidgetKind::HorizontalSubmodule)
        );
        assert_eq!(WidgetKind::parse("keybinds"), Some(WidgetKind::Keybinds));
        assert_eq!(WidgetKind::parse("monitors"), Some(WidgetKind::Monitors));
        assert_eq!(WidgetKind::parse("theme"), Some(WidgetKind::Theme));
        assert_eq!(WidgetKind::parse("wallpaper"), Some(WidgetKind::Wallpaper));
        assert_eq!(WidgetKind::parse("menu"), Some(WidgetKind::Menu));
        assert_eq!(WidgetKind::parse("plugins"), Some(WidgetKind::Plugins));
        assert_eq!(WidgetKind::parse("bar"), Some(WidgetKind::Bar));
        assert_eq!(WidgetKind::parse("dock"), Some(WidgetKind::Dock));
        assert_eq!(WidgetKind::parse("no-such-widget"), None);
    }

    #[test]
    fn unknown_widget_name_yields_none_spec() {
        let d = Directives::parse("widget = \"no-such-widget\"; label = \"X\";").unwrap();
        let opt = OptionSpec::from_directives(&d);
        assert_eq!(opt.widget, None);
        assert_eq!(opt.label.as_deref(), Some("X"));
    }

    #[test]
    fn parses_table_item_marker() {
        let (kind, d) =
            parse_comment("/* @table-item label = \"Main\"; hidden = true; */").unwrap();
        assert_eq!(kind, MarkerKind::TableItem);
        let spec = OptionSpec::from_directives(&d);
        assert_eq!(spec.label.as_deref(), Some("Main"));
        assert!(spec.hidden);
    }
}

/// A widget preset defined by an `@widget-preset` marker (for the add-picker).
#[derive(Debug, Clone, Default)]
pub struct WidgetPresetSpec {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub sections: Vec<String>,
    pub has_settings: bool,
    pub defaults: Vec<(String, Lit)>,
}

impl WidgetPresetSpec {
    pub fn from_directives(d: &Directives) -> Self {
        Self {
            id: d.string("id").unwrap_or_default(),
            label: d.string("label").unwrap_or_default(),
            description: d.string("description"),
            icon: d.string("icon"),
            sections: d.strings("sections").unwrap_or_default(),
            has_settings: d.bool("hasSettings").unwrap_or(false),
            defaults: d.sub("defaults").iter().cloned().collect(),
        }
    }
}

/// A field spec for a widget preset, defined by `@widget-preset-field`.
#[derive(Debug, Clone, Default)]
pub struct PresetFieldSpec {
    pub preset: String,
    pub name: String,
    pub widget: Option<WidgetKind>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub choices: Option<Vec<String>>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub unit: Option<String>,
}

impl PresetFieldSpec {
    pub fn from_directives(d: &Directives) -> Self {
        Self {
            preset: d.string("preset").unwrap_or_default(),
            name: d.string("name").unwrap_or_default(),
            widget: d.string("widget").and_then(|w| WidgetKind::parse(&w)),
            label: d.string("label"),
            description: d.string("description"),
            choices: d.strings("choices"),
            min: d.number("min"),
            max: d.number("max"),
            step: d.number("step"),
            unit: d.string("unit"),
        }
    }
}
