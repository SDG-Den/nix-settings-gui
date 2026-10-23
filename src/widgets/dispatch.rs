use super::bar::{build_bar, build_dock};
use super::basic::{
    build_color, build_dropdown, build_entry, build_list, build_multiline, build_package_editor,
    build_raw, build_slider, build_spin, build_switch,
};
use super::context::{BuildContext, ModuleKind};
use super::editor::Editor;
use super::grid::build_grid_configuration;
use super::keybinds::build_keybinds;
use super::menu::build_menu;
use super::module_list::build_module_list;
use super::monitors::build_monitors;
use super::nix::unwrap_modifiers;
use super::submodule::{build_horizontal_submodule, build_submodule};
use super::table::build_table;
use super::theme::build_theme;
use crate::apply::ApplyRequest;
use crate::dsl::{OptionSpec, WidgetKind};
use crate::model::Record;
use crate::value;
use std::rc::Rc;
/// Build the appropriate control for a record. `type_str` is the nixd type when
/// it is known; the `@option widget` takes precedence, then the type, then the
/// shape of the current value.
pub fn build(record: &Record, type_str: Option<&str>, ctx: &BuildContext) -> Editor {
    let editor = build_editor(record, type_str, ctx);
    connect_apply(record, &editor, ctx);
    editor
}

/// The record's path, prefixed with the path of the option that holds it, so
/// a nested option reports its full path.
pub(crate) fn full_path(record: &Record, ctx: &BuildContext) -> String {
    if ctx.path_prefix.is_empty() {
        record.path()
    } else {
        format!("{}.{}", ctx.path_prefix, record.path())
    }
}

/// Wire an option's apply command: every change runs the command with the
/// option's new value. The editor is cloned as a snapshot, which shares the
/// widgets, so the snapshot reads the value the user just entered.
fn connect_apply(record: &Record, editor: &Editor, ctx: &BuildContext) {
    let Some(command) = record.spec.apply_command.clone() else {
        return;
    };
    let Some(hook) = ctx.apply.clone() else {
        return;
    };
    if matches!(
        editor,
        Editor::Submodule { .. } | Editor::HorizontalSubmodule { .. }
    ) {
        return;
    }
    let snapshot = editor.clone();
    let path = full_path(record, ctx);
    let update: Rc<dyn Fn()> = Rc::new(move || {
        hook(ApplyRequest {
            path: path.clone(),
            command: command.clone(),
            value: snapshot.to_nix(),
        });
    });
    editor.connect_changed(&update);
}

/// Build the control for a record.
fn build_editor(record: &Record, type_str: Option<&str>, ctx: &BuildContext) -> Editor {
    let spec = &record.spec;
    let value_text = unwrap_modifiers(&record.value);
    let value_text = value_text.as_str();
    match choose_widget(record, type_str) {
        WidgetKind::Switch => Editor::Switch(build_switch(value_text)),
        WidgetKind::Dropdown | WidgetKind::Radio => {
            let choices = resolve_choices(spec, type_str);
            build_dropdown(&choices, value_text)
        }
        WidgetKind::Spin => Editor::Spin(build_spin(spec, value_text)),
        WidgetKind::Slider => Editor::Slider(build_slider(spec, value_text)),
        WidgetKind::Text => Editor::Text(build_entry(value_text, spec, false)),
        WidgetKind::Multiline => {
            let (view, root) = build_multiline(value_text, spec);
            Editor::Multiline { view, root }
        }
        WidgetKind::Path => Editor::Path(build_entry(value_text, spec, false)),
        WidgetKind::Package => build_package_editor(value_text, spec, ctx.search.as_ref()),
        WidgetKind::Packages => build_list(value_text, true, ctx.search.as_ref()),
        WidgetKind::List => build_list(value_text, false, None),
        WidgetKind::NixosModules => {
            build_module_list(value_text, ModuleKind::Nixos, &ctx.nixos_modules, record)
        }
        WidgetKind::HmModules => {
            build_module_list(value_text, ModuleKind::Home, &ctx.hm_modules, record)
        }
        WidgetKind::Color => Editor::Color(build_color(value_text)),
        WidgetKind::Submodule => build_submodule(record, type_str, ctx),
        WidgetKind::HorizontalSubmodule => build_horizontal_submodule(record, type_str, ctx),
        WidgetKind::GridConfiguration => build_grid_configuration(record, ctx),
        WidgetKind::TableAttrset => build_table(record, ctx, false),
        WidgetKind::TableList => build_table(record, ctx, true),
        WidgetKind::Theme => build_theme(value_text),
        WidgetKind::Keybinds => build_keybinds(record),
        WidgetKind::Monitors => build_monitors(record),
        WidgetKind::Menu => build_menu(record, ctx),
        WidgetKind::Plugins => crate::plugin_list::build(record, ctx),
        WidgetKind::Bar => build_bar(record, ctx),
        WidgetKind::Dock => build_dock(record, ctx),
        WidgetKind::Raw => {
            let (view, root) = build_raw(value_text, spec);
            Editor::Raw { view, root }
        }
    }
}

/// Whether a record is annotated as a single package or a package list.
pub fn is_package(record: &Record) -> bool {
    matches!(
        record.spec.widget,
        Some(WidgetKind::Package) | Some(WidgetKind::Packages)
    )
}

pub(crate) fn choose_widget(record: &Record, type_str: Option<&str>) -> WidgetKind {
    let spec = &record.spec;
    let value_text = &record.value;
    if let Some(widget) = spec.widget {
        if widget == WidgetKind::Spin
            && spec.min.is_some()
            && spec.max.is_some()
            && spec.horizontal
        {
            return WidgetKind::Slider;
        }
        if matches!(widget, WidgetKind::List | WidgetKind::Packages)
            && value::list_items(value_text).is_none()
        {
            return WidgetKind::Raw;
        }
        return widget;
    }
    if let Some(widget) = import_widget_kind(record) {
        return widget;
    }
    if let Some(widget) = type_str.and_then(widget_from_type) {
        return widget;
    }
    widget_from_kind(value::classify(value_text))
}

/// A top-level `imports` list becomes a dedicated module-import control: NixOS
/// modules for a `modules.nixos.*` origin, home-manager modules for
/// `modules.homeManager.*`.
pub(crate) fn import_widget_kind(record: &Record) -> Option<WidgetKind> {
    if record.segments.len() != 1 || record.segments[0] != "imports" {
        return None;
    }
    if record.origin.starts_with("modules.nixos.") {
        Some(WidgetKind::NixosModules)
    } else if record.origin.starts_with("modules.homeManager.") {
        Some(WidgetKind::HmModules)
    } else {
        None
    }
}

pub(crate) fn widget_from_type(type_str: &str) -> Option<WidgetKind> {
    let t = type_str.to_lowercase();
    if t.contains("one of") {
        Some(WidgetKind::Dropdown)
    } else if t.contains("list") && t.contains("package") {
        Some(WidgetKind::Packages)
    } else if t.contains("list") {
        Some(WidgetKind::List)
    } else if t.contains("package") {
        Some(WidgetKind::Package)
    } else if t.contains("path") {
        Some(WidgetKind::Path)
    } else if t.contains("bool") {
        Some(WidgetKind::Switch)
    } else if t.contains("float") || t.contains("number") || t.contains("int") {
        Some(WidgetKind::Spin)
    } else if t.contains("submodule") || t.contains("attribute set") || t.contains("attrs") {
        Some(WidgetKind::Submodule)
    } else if t.contains("string") {
        Some(WidgetKind::Text)
    } else {
        None
    }
}

pub(crate) fn widget_from_kind(kind: value::ValueKind) -> WidgetKind {
    match kind {
        value::ValueKind::Bool => WidgetKind::Switch,
        value::ValueKind::Int | value::ValueKind::Float => WidgetKind::Spin,
        value::ValueKind::String => WidgetKind::Text,
        value::ValueKind::List => WidgetKind::List,
        value::ValueKind::AttrSet => WidgetKind::Submodule,
        _ => WidgetKind::Raw,
    }
}

pub(crate) fn resolve_choices(spec: &OptionSpec, type_str: Option<&str>) -> Vec<String> {
    if let Some(choices) = &spec.choices {
        return choices.clone();
    }
    type_str.and_then(enum_choices).unwrap_or_default()
}

/// Pull the quoted members out of a `one of "a", "b"` type clause.
pub(crate) fn enum_choices(type_str: &str) -> Option<Vec<String>> {
    let lower = type_str.to_lowercase();
    let index = lower.find("one of")?;
    let mut out = Vec::new();
    let mut chars = type_str[index..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            let mut item = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                item.push(c);
            }
            out.push(item);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_enum_choices_from_type() {
        assert_eq!(
            enum_choices("one of \"a\", \"b\", \"c\""),
            Some(vec!["a".into(), "b".into(), "c".into()])
        );
        assert_eq!(enum_choices("list of string"), None);
    }
    #[test]
    fn type_mapping_covers_common_kinds() {
        assert_eq!(widget_from_type("boolean"), Some(WidgetKind::Switch));
        assert_eq!(
            widget_from_type("list of package"),
            Some(WidgetKind::Packages)
        );
        assert_eq!(widget_from_type("path"), Some(WidgetKind::Path));
        assert_eq!(widget_from_type("list of string"), Some(WidgetKind::List));
        assert_eq!(widget_from_type("freeform"), None);
    }
    #[test]
    fn list_over_non_list_value_falls_back_to_raw() {
        let mut record = Record {
            value: "builtins.attrValues self.modules.homeManager".to_string(),
            segments: vec!["home-manager".to_string(), "sharedModules".to_string()],
            ..Record::default()
        };
        record.spec.widget = Some(WidgetKind::List);
        assert_eq!(choose_widget(&record, None), WidgetKind::Raw);

        record.value = "[ git vim ]".to_string();
        assert_eq!(choose_widget(&record, None), WidgetKind::List);
    }
    #[test]
    fn bounded_spin_promotes_to_slider_only_when_horizontal() {
        let mut record = Record {
            value: "16".to_string(),
            ..Record::default()
        };
        record.spec.widget = Some(WidgetKind::Spin);
        record.spec.min = Some(16.0);
        record.spec.max = Some(96.0);
        assert_eq!(choose_widget(&record, None), WidgetKind::Spin);

        record.spec.horizontal = true;
        assert_eq!(choose_widget(&record, None), WidgetKind::Slider);
    }
    #[test]
    fn maps_imports_to_module_widgets() {
        let mut record = Record {
            segments: vec!["imports".to_string()],
            origin: "modules.nixos.audio".to_string(),
            ..Record::default()
        };
        assert_eq!(import_widget_kind(&record), Some(WidgetKind::NixosModules));

        record.origin = "modules.homeManager.desktop".to_string();
        assert_eq!(import_widget_kind(&record), Some(WidgetKind::HmModules));

        record.origin = String::new();
        assert_eq!(import_widget_kind(&record), None);

        record.origin = "modules.nixos.audio".to_string();
        record.segments = vec!["services".to_string(), "imports".to_string()];
        assert_eq!(import_widget_kind(&record), None);
    }
    #[test]
    fn parses_bespoke_widget_names() {
        assert_eq!(WidgetKind::parse("keybinds"), Some(WidgetKind::Keybinds));
        assert_eq!(WidgetKind::parse("monitors"), Some(WidgetKind::Monitors));
        assert_eq!(WidgetKind::parse("theme"), Some(WidgetKind::Theme));
        assert_eq!(WidgetKind::parse("menu"), Some(WidgetKind::Menu));
        assert_eq!(WidgetKind::parse("plugins"), Some(WidgetKind::Plugins));
        assert_eq!(WidgetKind::parse("bar"), Some(WidgetKind::Bar));
        assert_eq!(WidgetKind::parse("dock"), Some(WidgetKind::Dock));
        assert_eq!(WidgetKind::parse("nope"), None);
    }
}
