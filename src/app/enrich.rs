use std::collections::HashMap;
use std::rc::Rc;

use crate::dsl::WidgetKind;
use crate::metadata::{self, Target};
use crate::nixd::OptionInfo;
use crate::widgets::{self, Editor};

use super::controls::{build_context, connect_dirty};
use super::pump::spawn;
use super::state::{App, Msg, Tx};

// ---------------------------------------------------------------------------
// nixd enrichment
// ---------------------------------------------------------------------------

/// Split an option path's segments into its parent path and its own name.
fn parent_segments(segments: &[String]) -> Option<(Vec<String>, String)> {
    let (name, parent) = segments.split_last()?;
    Some((parent.to_vec(), name.clone()))
}

pub(super) fn query_current_page(app: &Rc<App>, tx: &Tx) {
    let file = app.current_file.borrow().clone();
    let target = app.target.borrow().clone();
    let base = app.base.borrow().clone();
    if target.is_empty() {
        set_nixd_status(app, "nixd: no option target for this config");
        return;
    }
    let paths: Vec<(String, Vec<String>)> = app
        .pages
        .borrow()
        .iter()
        .find(|page| page.file == file)
        .map(|page| {
            page.controls
                .iter()
                .map(|c| (c.record.path(), c.record.path_segments().to_vec()))
                .collect()
        })
        .unwrap_or_default();
    set_nixd_status(app, "Querying nixd...");
    spawn(tx, move |tx| {
        let parsed = Target::parse(&target);
        let mut parents: HashMap<String, Vec<metadata::ChildOption>> = HashMap::new();
        let mut infos = Vec::new();
        for (path, segments) in paths {
            if let Ok(info) = crate::nixd::query_option(&base, &target, &path) {
                if info.type_str.is_some() || !info.description.is_empty() {
                    infos.push((path, info));
                    continue;
                }
            }
            let (Some(target), Some((parent, name))) =
                (parsed.as_ref(), parent_segments(&segments))
            else {
                continue;
            };
            let key = parent.join(".");
            let children = parents
                .entry(key)
                .or_insert_with(|| metadata::enumerate(&base, target, &parent).unwrap_or_default());
            if let Some(child) = children.iter().find(|child| child.name == name) {
                infos.push((
                    path,
                    OptionInfo {
                        type_str: Some(child.type_str.clone()),
                        description: child.description.clone(),
                    },
                ));
            }
        }
        let _ = tx.send_blocking(Msg::Infos(file.clone(), infos));
    });
}

pub(super) fn apply_infos(app: &Rc<App>, file: &str, infos: &[(String, OptionInfo)]) {
    let mut updated = 0;
    for page in app.pages.borrow().iter() {
        if page.file != file {
            continue;
        }
        for control in &page.controls {
            let Some((_, info)) = infos
                .iter()
                .find(|(path, _)| *path == control.record.path())
            else {
                continue;
            };
            if !info.description.is_empty() {
                control.details.set_description(&info.description);
            }
            if let Some(type_str) = &info.type_str {
                if let Some(widget) = type_widget(type_str) {
                    let needs_rebuild = match &*control.editor.borrow() {
                        Editor::Raw { .. } => widget != WidgetKind::Raw,
                        _ => false,
                    };
                    if needs_rebuild && control.editor.borrow().unchanged(&control.record.value) {
                        let editor =
                            widgets::build(&control.record, Some(type_str), &build_context(app));
                        control.set_editor(editor);
                        connect_dirty(app, control);
                    }
                }
            }
            updated += 1;
        }
    }
    set_nixd_status(app, &format!("nixd updated {updated} option(s)"));
}

fn set_nixd_status(app: &Rc<App>, message: &str) {
    app.nixd_banner.set_title(message);
    app.nixd_banner.set_revealed(!message.is_empty());
}

fn type_widget(type_str: &str) -> Option<WidgetKind> {
    let lower = type_str.to_lowercase();
    if lower.contains("one of") {
        Some(WidgetKind::Dropdown)
    } else if lower.contains("bool") {
        Some(WidgetKind::Switch)
    } else if lower.contains("float") || lower.contains("number") || lower.contains("int") {
        Some(WidgetKind::Spin)
    } else if lower.contains("path") {
        Some(WidgetKind::Path)
    } else if lower.contains("package") {
        Some(WidgetKind::Package)
    } else if lower.contains("list") {
        Some(WidgetKind::List)
    } else if lower.contains("string") {
        Some(WidgetKind::Text)
    } else {
        Some(WidgetKind::Raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_option_paths() {
        let segments = vec!["services".to_string(), "openssh".to_string()];
        assert_eq!(
            parent_segments(&segments),
            Some((vec!["services".to_string()], "openssh".to_string()))
        );
        assert_eq!(
            parent_segments(&segments[..1]),
            Some((Vec::new(), "services".to_string()))
        );
        assert_eq!(parent_segments(&[]), None);
    }
}
