use crate::widgets::nix::quote;

/// The editor shape inferred from a DMS declarative settings control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SchemaKind {
    Bool,
    Int,
    Float,
    Text,
    Dropdown(Vec<String>),
    Color,
    Raw,
}

/// One setting declared by a plugin's Settings.qml.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SchemaField {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) kind: SchemaKind,
    /// Nix literal used when the configuration has no value for the key.
    pub(crate) default: String,
    pub(crate) minimum: Option<String>,
    pub(crate) maximum: Option<String>,
}

const COMPONENTS: &[&str] = &[
    "ToggleSetting",
    "StringSetting",
    "SliderSetting",
    "SelectionSetting",
    "ColorSetting",
    "ListSetting",
    "ListSettingWithInput",
];

/// Extract the declarative controls from a DMS plugin settings QML file.
/// Unknown or custom QML yields no fields; the caller keeps its fallback.
pub(crate) fn parse_settings(qml: &str) -> Vec<SchemaField> {
    let mut fields: Vec<SchemaField> = Vec::new();
    for (component, block) in component_blocks(qml) {
        let Some(field) = field_for(component, block) else {
            continue;
        };
        if fields.iter().any(|existing| existing.key == field.key) {
            continue;
        }
        fields.push(field);
    }
    fields
}

fn field_for(component: &str, block: &str) -> Option<SchemaField> {
    let key = property(block, "settingKey").and_then(|value| first_string(&value))?;
    let label = property(block, "label")
        .and_then(|value| first_string(&value))
        .unwrap_or_default();
    let description = property(block, "description")
        .and_then(|value| first_string(&value))
        .unwrap_or_default();
    let default = property(block, "defaultValue");

    let (kind, default, minimum, maximum) = match component {
        "ToggleSetting" => (
            SchemaKind::Bool,
            bool_literal(default.as_deref()).to_string(),
            None,
            None,
        ),
        "StringSetting" => (
            SchemaKind::Text,
            string_literal(default.as_deref()),
            None,
            None,
        ),
        "SliderSetting" => (
            SchemaKind::Int,
            number_literal(default.as_deref(), true),
            number_option(block, "minimum"),
            number_option(block, "maximum"),
        ),
        "SelectionSetting" => {
            let choices = property(block, "options")
                .map(|options| choice_values(&options))
                .unwrap_or_default();
            (
                SchemaKind::Dropdown(choices),
                string_literal(default.as_deref()),
                None,
                None,
            )
        }
        "ColorSetting" => (
            SchemaKind::Color,
            color_literal(default.as_deref()),
            None,
            None,
        ),
        "ListSetting" | "ListSettingWithInput" => (
            SchemaKind::Raw,
            list_literal(default.as_deref()),
            None,
            None,
        ),
        _ => return None,
    };

    Some(SchemaField {
        key,
        label,
        description,
        kind,
        default,
        minimum,
        maximum,
    })
}

fn bool_literal(value: Option<&str>) -> &'static str {
    if value.is_some_and(|text| text.trim() == "true") {
        "true"
    } else {
        "false"
    }
}

fn string_literal(value: Option<&str>) -> String {
    value
        .and_then(first_string)
        .map(|text| quote(&text))
        .unwrap_or_else(|| "\"\"".to_string())
}

fn color_literal(value: Option<&str>) -> String {
    value
        .and_then(first_string)
        .map(|text| quote(&text))
        .unwrap_or_else(|| "\"#000000\"".to_string())
}

fn number_literal(value: Option<&str>, integer: bool) -> String {
    let Some(value) = value else {
        return "0".to_string();
    };
    let text = value.trim();
    if integer {
        text.parse::<i64>()
            .map(|number| number.to_string())
            .unwrap_or_else(|_| "0".to_string())
    } else {
        text.parse::<f64>()
            .map(|number| number.to_string())
            .unwrap_or_else(|_| "0".to_string())
    }
}

fn number_option(block: &str, name: &str) -> Option<String> {
    let value = property(block, name)?;
    value
        .trim()
        .parse::<f64>()
        .ok()
        .map(|number| number.to_string())
}

fn list_literal(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "[]".to_string();
    };
    if value.contains('{') {
        return "[]".to_string();
    }
    let items: Vec<String> = all_strings(value).iter().map(|item| quote(item)).collect();
    if items.is_empty() {
        "[]".to_string()
    } else {
        format!("[ {} ]", items.join(" "))
    }
}

fn choice_values(value: &str) -> Vec<String> {
    let entries = string_entries(value);
    let explicit: Vec<String> = entries
        .iter()
        .enumerate()
        .filter(|(index, (text, end))| {
            *text == "value"
                && value[*end..].trim_start().starts_with(':')
                && entries.get(index + 1).is_some()
        })
        .filter_map(|(index, _)| entries.get(index + 1).map(|(text, _)| text.clone()))
        .collect();
    if !explicit.is_empty() {
        return explicit;
    }
    entries.into_iter().map(|(text, _)| text).collect()
}

fn component_blocks(src: &str) -> Vec<(&str, &str)> {
    let bytes = src.as_bytes();
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(next) = skip_literal(bytes, index) {
            index = next;
            continue;
        }
        if !is_ident_start(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < bytes.len() && is_ident_continue(bytes[index]) {
            index += 1;
        }
        let name = &src[start..index];
        if !COMPONENTS.contains(&name) {
            continue;
        }
        let mut open = index;
        while open < bytes.len() && bytes[open].is_ascii_whitespace() {
            open += 1;
        }
        if open >= bytes.len() || bytes[open] != b'{' {
            continue;
        }
        let Some(close) = matching_brace(bytes, open) else {
            continue;
        };
        blocks.push((name, &src[open + 1..close]));
        index = close + 1;
    }
    blocks
}

fn matching_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut index = open;
    while index < bytes.len() {
        if let Some(next) = skip_literal(bytes, index) {
            index = next;
            continue;
        }
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn property(block: &str, name: &str) -> Option<String> {
    let bytes = block.as_bytes();
    let mut index = 0;
    let mut depth = 0i32;
    while index < bytes.len() {
        if let Some(next) = skip_literal(bytes, index) {
            index = next;
            continue;
        }
        match bytes[index] {
            b'{' | b'[' | b'(' => {
                depth += 1;
                index += 1;
                continue;
            }
            b'}' | b']' | b')' => {
                depth -= 1;
                index += 1;
                continue;
            }
            byte if depth == 0 && is_ident_start(byte) => {
                let start = index;
                index += 1;
                while index < bytes.len() && is_ident_continue(bytes[index]) {
                    index += 1;
                }
                if &block[start..index] != name {
                    continue;
                }
                let mut colon = index;
                while colon < bytes.len() && matches!(bytes[colon], b' ' | b'\t' | b'\r') {
                    colon += 1;
                }
                if colon >= bytes.len() || bytes[colon] != b':' {
                    continue;
                }
                colon += 1;
                while colon < bytes.len() && matches!(bytes[colon], b' ' | b'\t' | b'\r') {
                    colon += 1;
                }
                let value_start = colon;
                let mut nested = 0i32;
                let mut value_end = colon;
                while value_end < bytes.len() {
                    if let Some(next) = skip_literal(bytes, value_end) {
                        value_end = next;
                        continue;
                    }
                    match bytes[value_end] {
                        b'{' | b'[' | b'(' => nested += 1,
                        b'}' | b']' | b')' => nested -= 1,
                        b'\n' | b';' if nested == 0 => break,
                        _ => {}
                    }
                    value_end += 1;
                }
                let value = block[value_start..value_end].trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn first_string(text: &str) -> Option<String> {
    for (offset, ch) in text.char_indices() {
        if ch == '"' || ch == '\'' {
            return read_string(text, offset, ch).map(|(value, _)| value);
        }
    }
    None
}

fn all_strings(text: &str) -> Vec<String> {
    string_entries(text)
        .into_iter()
        .map(|(value, _)| value)
        .collect()
}

fn string_entries(text: &str) -> Vec<(String, usize)> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let Some((offset, ch)) = text[index..]
            .char_indices()
            .find(|(_, ch)| *ch == '"' || *ch == '\'')
        else {
            break;
        };
        let start = index + offset;
        let Some((value, end)) = read_string(text, start, ch) else {
            break;
        };
        values.push((value, end));
        index = end;
    }
    values
}

fn read_string(text: &str, start: usize, quote: char) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = text[start + quote.len_utf8()..].char_indices();
    while let Some((offset, ch)) = chars.next() {
        if ch == quote {
            let end = start + quote.len_utf8() + offset + ch.len_utf8();
            return Some((out, end));
        }
        if ch == '\\' {
            let (_, escaped) = chars.next()?;
            match escaped {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                other => out.push(other),
            }
        } else {
            out.push(ch);
        }
    }
    None
}

fn skip_literal(bytes: &[u8], index: usize) -> Option<usize> {
    match bytes[index] {
        b'"' | b'\'' => {
            let quote = bytes[index];
            let mut cursor = index + 1;
            while cursor < bytes.len() {
                if bytes[cursor] == b'\\' {
                    cursor += 2;
                    continue;
                }
                if bytes[cursor] == quote {
                    return Some(cursor + 1);
                }
                cursor += 1;
            }
            Some(bytes.len())
        }
        b'/' if bytes.get(index + 1) == Some(&b'/') => {
            let mut cursor = index + 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            Some(cursor)
        }
        b'/' if bytes.get(index + 1) == Some(&b'*') => {
            let mut cursor = index + 2;
            while cursor + 1 < bytes.len() {
                if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                    return Some(cursor + 2);
                }
                cursor += 1;
            }
            Some(bytes.len())
        }
        _ => None,
    }
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_bar_dropdown_settings() {
        let qml = r#"
            PluginSettings {
                id: root
                ToggleSetting {
                    settingKey: "showChevron"
                    label: I18n.tr("Show chevron")
                    description: I18n.tr("A small arrow beside the icon.")
                    defaultValue: true
                }
                SelectionSetting {
                    settingKey: "display"
                    label: I18n.tr("Button contents")
                    defaultValue: "icon"
                    options: [
                        { "label": "Icon only", "value": "icon" },
                        { "label": "Icon and label", "value": "both" }
                    ]
                }
                StringSetting {
                    settingKey: "label"
                    label: "Button label"
                    defaultValue: ""
                }
                ListSettingWithInput {
                    settingKey: "targets"
                    label: "Panel widgets"
                    defaultValue: []
                    fields: [ { "id": "id" } ]
                }
            }
        "#;
        let fields = parse_settings(qml);
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0].key, "showChevron");
        assert_eq!(fields[0].kind, SchemaKind::Bool);
        assert_eq!(fields[0].default, "true");
        assert_eq!(fields[1].key, "display");
        assert_eq!(
            fields[1].kind,
            SchemaKind::Dropdown(vec!["icon".into(), "both".into()])
        );
        assert_eq!(fields[1].default, "\"icon\"");
        assert_eq!(fields[2].default, "\"\"");
        assert_eq!(fields[3].kind, SchemaKind::Raw);
        assert_eq!(fields[3].default, "[]");
    }

    #[test]
    fn parses_sliders_and_ignores_unknown_components() {
        let qml = r##"
            PluginSettings {
                SliderSetting {
                    settingKey: "toolbarOpacity"
                    label: "Toolbar opacity"
                    defaultValue: 80
                    minimum: 0
                    maximum: 100
                }
                CustomRow {
                    settingKey: "ignored"
                }
                ColorSetting {
                    settingKey: "accent"
                    label: "Accent"
                    defaultValue: "#ff0000"
                }
            }
        "##;
        let fields = parse_settings(qml);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].kind, SchemaKind::Int);
        assert_eq!(fields[0].default, "80");
        assert_eq!(fields[0].minimum.as_deref(), Some("0"));
        assert_eq!(fields[0].maximum.as_deref(), Some("100"));
        assert_eq!(fields[1].kind, SchemaKind::Color);
        assert_eq!(fields[1].default, "\"#ff0000\"");
    }

    #[test]
    fn custom_settings_qml_has_no_fields() {
        let qml = r#"
            FocusScope {
                id: root
                property var pluginService: null
                function save(key, value) { pluginService.savePluginData("x", key, value); }
            }
        "#;
        assert!(parse_settings(qml).is_empty());
    }

    #[test]
    fn comments_and_strings_do_not_confuse_the_block_scanner() {
        let qml = r#"
            // ToggleSetting { settingKey: "notReal" }
            PluginSettings {
                StringSetting {
                    settingKey: "path"
                    label: "Menu } file"
                    description: "Contains { braces } and \"quotes\"."
                    defaultValue: "a\"b"
                }
            }
        "#;
        let fields = parse_settings(qml);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "path");
        assert_eq!(fields[0].description, "Contains { braces } and \"quotes\".");
        assert_eq!(fields[0].default, "\"a\\\"b\"");
    }

    #[test]
    #[ignore = "needs a sibling ojuny-template checkout and Nix evaluation"]
    fn parses_the_real_bar_dropdown_settings() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ojuny-template");
        if !path.join("flake.nix").exists() {
            return;
        }
        let qml = crate::metadata::dms_plugin_settings(&path.to_string_lossy(), "barDropdown")
            .expect("the registry plugin should load")
            .expect("barDropdown ships settings");
        let fields = parse_settings(&qml);
        let keys: Vec<&str> = fields.iter().map(|field| field.key.as_str()).collect();
        assert!(keys.contains(&"targets"), "{keys:?}");
        assert!(keys.contains(&"showChevron"), "{keys:?}");
        assert!(keys.contains(&"display"), "{keys:?}");
    }
}
