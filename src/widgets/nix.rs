use crate::dsl::WidgetKind;
use crate::value;
use std::path::{Component, Path, PathBuf};

/// Quote a value as a double-quoted Nix string. Backslashes, quotes, and
/// interpolation openers are escaped so the result never gains meaning.
pub fn quote(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${");
    format!("\"{escaped}\"")
}

/// A string that contains a newline is written as an indented `''` string.
pub(crate) fn string_literal(s: &str) -> String {
    if s.contains('\n') {
        format!("''\n{s}\n''")
    } else {
        quote(s)
    }
}

/// Strip value modifiers (`lib.mkDefault`, `lib.mkForce`, `lib.mkOverride <n>`)
/// so the control compares and edits the value that is actually assigned.
pub(crate) fn unwrap_modifiers(text: &str) -> String {
    let mut t = text.trim();
    loop {
        let rest = t
            .strip_prefix("lib.mkDefault")
            .or_else(|| t.strip_prefix("lib.mkForce"))
            .or_else(|| {
                t.strip_prefix("lib.mkOverride").map(|r| {
                    r.trim_start()
                        .trim_start_matches(|c: char| c.is_ascii_digit())
                })
            });
        match rest.map(str::trim) {
            Some(r) if !r.is_empty() => t = r,
            _ => break,
        }
    }
    t.to_string()
}

/// Nix truthiness: only `false`, `null`, and the numeric zero forms are falsy.
pub(crate) fn truthy(value: &str) -> bool {
    !matches!(
        unwrap_modifiers(value).trim(),
        "false" | "null" | "0" | "0.0"
    )
}

/// A dropdown or radio choice: numbers and booleans stay verbsatim, anything
/// else is quoted, because `choices` in the DSL are lists of strings.
pub(crate) fn choice_literal(raw: &str) -> String {
    let t = raw.trim();
    if matches!(t, "true" | "false" | "null") || t.parse::<f64>().is_ok() {
        t.to_string()
    } else {
        quote(t)
    }
}

/// A list element or package reference: keep the text verbatim when it is a
/// valid Nix expression (`pkgs.git`, `"nix-command"`), otherwise quote it.
pub(crate) fn expr_literal(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return "\"\"".to_string();
    }
    let parsed = rnix::Root::parse(t).ok().ok().and_then(|r| r.expr());
    if parsed.is_some() {
        t.to_string()
    } else {
        quote(t)
    }
}

/// A package entry that is a bare attribute name (`git`, `python3`, `gcc-13`)
/// rather than a qualified expression (`pkgs.git`).
pub(crate) fn is_bare_package(text: &str) -> bool {
    let t = text.trim();
    match t.chars().next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    t.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Serialize a package list as the editor writes it into the existing value
/// span. The span is the inner list (the file's own `with pkgs;` sits outside
/// it), so this stays a plain list and never adds a wrapper.
pub(crate) fn package_items(entries: &[String]) -> String {
    let items: Vec<String> = entries
        .iter()
        .map(|e| expr_literal(e))
        .filter(|e| !e.is_empty() && e != "\"\"")
        .collect();
    value::serialize_list(&items)
}

/// Serialize a standalone package assignment. Bare names need `pkgs` in scope,
/// so they go under `with pkgs;`; already-qualified expressions stay verbatim.
/// Used when inserting a brand-new option, which has no wrapper of its own.
pub(crate) fn serialize_packages(entries: &[String]) -> String {
    let cleaned: Vec<String> = entries
        .iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();
    if !cleaned.is_empty() && cleaned.iter().all(|e| is_bare_package(e)) {
        format!("with pkgs; {}", package_items(entries))
    } else {
        package_items(entries)
    }
}

/// Serialize a module list, prefixing each name with the scope qualifier the
/// value already used (e.g. `self.modules.nixos.`), or none for bare names.
pub(crate) fn serialize_modules(names: &[String], prefix: &str) -> String {
    let items: Vec<String> = names
        .iter()
        .map(|n| format!("{prefix}{}", n.trim()))
        .collect();
    value::serialize_list(&items)
}

/// Canonical form of a list value as the editor would write it. Comparing this
/// against `to_nix` ignores source formatting (multi-line, extra spacing), so
/// an untouched list is never mistaken for a change.
pub(crate) fn canonical_list(original: &str, packages: bool) -> Option<String> {
    let items = value::list_items(original)?;
    if packages {
        Some(package_items(&items))
    } else {
        let mapped: Vec<String> = items
            .iter()
            .map(|item| expr_literal(item))
            .filter(|item| !item.is_empty() && item != "\"\"")
            .collect();
        Some(value::serialize_list(&mapped))
    }
}

/// The module names a module control holds. A list literal yields its elements;
/// a bare single reference (e.g. `self.modules.homeManager.foo`) yields that one
/// expression.
pub(crate) fn module_items(text: &str) -> Option<Vec<String>> {
    value::list_items(text).or_else(|| value::single_expr(text).map(|e| vec![e]))
}

/// Canonical form of a module list. The editor stores each item with its scope
/// prefix stripped, so strip the same prefix before re-serializing.
pub(crate) fn canonical_module_list(original: &str, prefix: &str) -> Option<String> {
    let items = module_items(original)?;
    let names: Vec<String> = items
        .iter()
        .map(|item| {
            let raw = item.trim();
            if prefix.is_empty() {
                raw.to_string()
            } else {
                raw.strip_prefix(prefix).unwrap_or(raw).to_string()
            }
        })
        .filter(|name| !name.is_empty())
        .collect();
    Some(serialize_modules(&names, prefix))
}

/// A single package reference: a bare name is qualified with `pkgs.`.
pub(crate) fn package_literal(raw: &str) -> String {
    let t = raw.trim();
    if is_bare_package(t) {
        format!("pkgs.{t}")
    } else {
        expr_literal(t)
    }
}

pub(crate) fn number_literal(value: f64, digits: u32) -> String {
    if digits == 0 && value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        let precision = digits as usize;
        format!("{value:.precision$}")
    }
}

/// A path stays a bare path literal when it looks like one, else a string.
pub(crate) fn path_literal(raw: &str) -> String {
    let t = raw.trim();
    if t.starts_with("./") || t.starts_with("../") || t.starts_with('/') {
        t.to_string()
    } else {
        quote(t)
    }
}
/// Strip the surrounding quotes from a simple double-quoted string literal.
pub(crate) fn unescape(text: &str) -> String {
    let inner = &text[1..text.len() - 1];
    inner
        .replace("\\\"", "\"")
        .replace("\\${", "${")
        .replace("\\\\", "\\")
}

/// The text an entry shows for a value: a quoted literal is unquoted, anything
/// else is trimmed as-is.
pub(crate) fn entry_text(value: &str) -> String {
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        unescape(value)
    } else {
        value.trim().to_string()
    }
}

/// Decode a `"..."` or `''...''` string literal into its raw text.
pub(crate) fn parse_string_literal(text: &str) -> String {
    let t = text.trim();
    if let Some(inner) = t.strip_prefix("''").and_then(|s| s.strip_suffix("''")) {
        return inner.trim_matches('\n').to_string();
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        return unescape(t);
    }
    t.to_string()
}
/// A Nix attribute name, kept bare when it is a plain identifier. The value is
/// NOT trimmed: a key such as `"daily "` keeps its trailing space when quoted,
/// so a written binding still matches the key that was read.
pub(crate) fn attr_key(raw: &str) -> String {
    let valid = !raw.is_empty()
        && raw
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        raw.to_string()
    } else {
        quote(raw)
    }
}
/// Serialize a hand-entered value for a chosen widget kind. Used by the
/// add-option flow to build the assignment it inserts.
pub fn literal_for_widget(widget: WidgetKind, value: &str) -> String {
    let t = value.trim();
    match widget {
        WidgetKind::Switch => {
            if matches!(t, "true" | "false") {
                t.to_string()
            } else {
                "false".to_string()
            }
        }
        WidgetKind::Spin | WidgetKind::Slider => {
            if t.parse::<f64>().is_ok() {
                t.to_string()
            } else {
                "0".to_string()
            }
        }
        WidgetKind::Text | WidgetKind::Multiline | WidgetKind::Color => string_literal(value),
        WidgetKind::Path => path_literal(value),
        WidgetKind::Wallpaper => {
            if t == "null" {
                "null".to_string()
            } else {
                nix_path_literal(value)
            }
        }
        WidgetKind::Dropdown | WidgetKind::Radio => choice_literal(value),
        WidgetKind::Packages => {
            let items = value::list_items(value).unwrap_or_else(|| vec![value.trim().to_string()]);
            serialize_packages(&items)
        }
        WidgetKind::Package => package_literal(value),
        WidgetKind::List
        | WidgetKind::NixosModules
        | WidgetKind::HmModules
        | WidgetKind::Raw
        | WidgetKind::Submodule
        | WidgetKind::HorizontalSubmodule
        | WidgetKind::TableAttrset
        | WidgetKind::TableList
        | WidgetKind::Keybinds
        | WidgetKind::Monitors
        | WidgetKind::Theme
        | WidgetKind::Menu
        | WidgetKind::Plugins
        | WidgetKind::Bar
        | WidgetKind::Dock
        | WidgetKind::GridConfiguration => expr_literal(value),
    }
}

/// A path stays a bare path literal when it looks like one, else a string.
/// A relative path containing whitespace becomes a path-expression addition,
/// because a Nix path literal cannot contain spaces.
pub(crate) fn nix_path_literal(value: &str) -> String {
    if !value.chars().any(char::is_whitespace) {
        return path_literal(value);
    }
    if Path::new(value).is_absolute() {
        return quote(value);
    }
    match value.rsplit_once('/') {
        Some((parent, name)) => {
            let parent = match parent {
                "" | "." => "./.".to_string(),
                ".." => "../.".to_string(),
                other => other.to_string(),
            };
            format!("({parent} + {})", quote(&format!("/{name}")))
        }
        None => format!("(./. + {})", quote(&format!("/{value}"))),
    }
}

/// Resolve `.` and `..` components without touching the filesystem.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
            Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// Render `to` as a relative path from `from`, keeping the `./` prefix that
/// Nix treats as relative to the containing file.
pub(crate) fn relative_path(from: &Path, to: &Path) -> String {
    let from = normalize_path(from);
    let to = normalize_path(to);
    let from_parts: Vec<Component> = from.components().collect();
    let to_parts: Vec<Component> = to.components().collect();
    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..from_parts.len() {
        relative.push("..");
    }
    for part in &to_parts[common..] {
        relative.push(part.as_os_str());
    }
    let text = relative.to_string_lossy().into_owned();
    if text.starts_with("..") || Path::new(&text).is_absolute() {
        text
    } else {
        format!("./{text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_escapes_strings() {
        assert_eq!(quote("hello"), "\"hello\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("${x}"), "\"\\${x}\"");
    }

    #[test]
    fn multiline_uses_indented_string() {
        assert_eq!(string_literal("one\ntwo"), "''\none\ntwo\n''");
        assert_eq!(string_literal("one"), "\"one\"");
    }

    #[test]
    fn distinguishes_choice_literals() {
        assert_eq!(choice_literal("auto"), "\"auto\"");
        assert_eq!(choice_literal("true"), "true");
        assert_eq!(choice_literal("42"), "42");
    }

    #[test]
    fn keeps_valid_expressions_verbatim() {
        assert_eq!(expr_literal("pkgs.git"), "pkgs.git");
        assert_eq!(expr_literal("\"nix-command\""), "\"nix-command\"");
        assert_eq!(expr_literal("Europe/Amsterdam"), "Europe/Amsterdam");
        assert_eq!(expr_literal("two words"), "two words");
        assert_eq!(expr_literal("("), "\"(\"");
    }

    #[test]
    fn number_formatting_respects_digits() {
        assert_eq!(number_literal(8.0, 0), "8");
        assert_eq!(number_literal(1.5, 2), "1.50");
    }
    #[test]
    fn path_literals_are_recognised() {
        assert_eq!(path_literal("./foo"), "./foo");
        assert_eq!(path_literal("/etc/x"), "/etc/x");
        assert_eq!(path_literal("foo"), "\"foo\"");
    }

    #[test]
    fn whitespace_paths_become_path_expressions() {
        assert_eq!(
            nix_path_literal("./wallpapers/my paper.png"),
            "(./wallpapers + \"/my paper.png\")"
        );
        assert_eq!(
            nix_path_literal("../assets/my paper.png"),
            "(../assets + \"/my paper.png\")"
        );
        assert_eq!(
            nix_path_literal("my paper.png"),
            "(./. + \"/my paper.png\")"
        );
        assert_eq!(
            nix_path_literal("/tmp/my paper.png"),
            "\"/tmp/my paper.png\""
        );
    }

    #[test]
    fn serialises_add_option_values() {
        assert_eq!(literal_for_widget(WidgetKind::Switch, "true"), "true");
        assert_eq!(literal_for_widget(WidgetKind::Spin, "8"), "8");
        assert_eq!(literal_for_widget(WidgetKind::Text, "hello"), "\"hello\"");
        assert_eq!(
            literal_for_widget(WidgetKind::Packages, "[ pkgs.git pkgs.vim ]"),
            "[ pkgs.git pkgs.vim ]"
        );
        assert_eq!(
            literal_for_widget(WidgetKind::Wallpaper, "./wallpapers/my paper.png"),
            "(./wallpapers + \"/my paper.png\")"
        );
        assert_eq!(
            literal_for_widget(WidgetKind::Wallpaper, "everforest"),
            "\"everforest\""
        );
    }

    #[test]
    fn qualifies_single_bare_packages() {
        assert_eq!(literal_for_widget(WidgetKind::Package, "git"), "pkgs.git");
        assert_eq!(
            literal_for_widget(WidgetKind::Package, "pkgs.git"),
            "pkgs.git"
        );
        assert_eq!(package_literal("git"), "pkgs.git");
        assert_eq!(package_literal("pkgs.vim"), "pkgs.vim");
    }

    #[test]
    fn wraps_bare_packages_in_with_pkgs() {
        assert_eq!(
            literal_for_widget(WidgetKind::Packages, "[ git vim ]"),
            "with pkgs; [ git vim ]"
        );
        assert_eq!(
            serialize_packages(&["git".into(), "python3".into()]),
            "with pkgs; [ git python3 ]"
        );
        assert_eq!(
            serialize_packages(&["pkgs.git".into(), "vim".into()]),
            "[ pkgs.git vim ]"
        );
        assert!(is_bare_package("gcc-13"));
        assert!(!is_bare_package("pkgs.git"));
        assert!(!is_bare_package("git.override {}"));
    }

    #[test]
    fn canonicalises_multiline_lists() {
        let list = "[\n      \"nix-command\"\n      \"flakes\"\n    ]";
        assert_eq!(
            canonical_list(list, false),
            Some("[ \"nix-command\" \"flakes\" ]".to_string())
        );
        let packages = "[\n        git\n        vim\n      ]";
        assert_eq!(
            canonical_list(packages, true),
            Some("[ git vim ]".to_string())
        );
        assert_eq!(canonical_list("not a list", false), None);
    }

    #[test]
    fn package_edits_keep_the_existing_with_wrapper() {
        let original = "[ git vim ]";
        assert_eq!(canonical_list(original, true), Some(original.to_string()));
        assert_eq!(
            package_items(&["git".into(), "vim".into(), "neovim".into()]),
            "[ git vim neovim ]"
        );
        assert!(!package_items(&["git".into()]).contains("with pkgs;"));
    }
    #[test]
    fn canonicalises_module_lists_ignoring_scope() {
        let original = "[\n  self.modules.nixos.common\n  self.modules.nixos.audio\n]";
        assert_eq!(
            canonical_module_list(original, "self.modules.nixos."),
            Some("[ self.modules.nixos.common self.modules.nixos.audio ]".to_string())
        );
        assert_eq!(
            canonical_module_list("[ common ]", ""),
            Some("[ common ]".to_string())
        );
    }

    #[test]
    fn unwraps_value_modifiers() {
        assert_eq!(unwrap_modifiers("lib.mkDefault true"), "true");
        assert_eq!(unwrap_modifiers("lib.mkForce pkgs.git"), "pkgs.git");
        assert_eq!(unwrap_modifiers("lib.mkOverride 70 [ \"a\" ]"), "[ \"a\" ]");
        assert_eq!(unwrap_modifiers("  true  "), "true");
        assert_eq!(unwrap_modifiers("lib.mkDefault"), "lib.mkDefault");
    }

    #[test]
    fn reads_nix_truthiness() {
        assert!(truthy("true"));
        assert!(truthy("1"));
        assert!(truthy("lib.mkDefault true"));
        assert!(truthy("pkgs.linuxPackages_latest"));
        assert!(!truthy("false"));
        assert!(!truthy("0"));
        assert!(!truthy("null"));
    }

    #[test]
    fn canonicalises_single_module_reference() {
        let original = "self.modules.homeManager.homeconfig-den";
        assert_eq!(
            canonical_module_list(original, "self.modules.homeManager."),
            Some("[ self.modules.homeManager.homeconfig-den ]".to_string())
        );
        assert_eq!(
            module_items("[ a b ]"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            module_items("self.modules.nixos.foo"),
            Some(vec!["self.modules.nixos.foo".to_string()])
        );
        assert_eq!(module_items("1 + 2"), None);
    }

    #[test]
    fn package_and_entry_text_round_trip() {
        assert_eq!(entry_text("\"en_US.UTF-8\""), "en_US.UTF-8");
        assert_eq!(entry_text(" pkgs.git "), "pkgs.git");
        assert_eq!(package_literal(&entry_text("\"git\"")), "pkgs.git");
        assert_eq!(package_literal(&entry_text("pkgs.git")), "pkgs.git");
    }

    #[test]
    fn serialises_module_lists() {
        let names = vec!["audio".to_string(), "desktop".to_string()];
        assert_eq!(
            serialize_modules(&names, "flake.modules.nixos."),
            "[ flake.modules.nixos.audio flake.modules.nixos.desktop ]"
        );
        assert_eq!(
            serialize_modules(&names, "self.modules.homeManager."),
            "[ self.modules.homeManager.audio self.modules.homeManager.desktop ]"
        );
        assert_eq!(serialize_modules(&names, ""), "[ audio desktop ]");
        assert_eq!(serialize_modules(&[], "flake.modules.nixos."), "[]");
    }
    #[test]
    fn quotes_dotted_attr_keys() {
        assert_eq!(attr_key("dank-rs.cal.simple"), "\"dank-rs.cal.simple\"");
        assert_eq!(attr_key("plain"), "plain");
        assert_eq!(attr_key(""), "\"\"");
        assert_eq!(attr_key("daily "), "\"daily \"");
    }
}
