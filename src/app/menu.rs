use std::cmp::Ordering;
use std::rc::Rc;

use gtk::prelude::*;

use crate::model::MenuModel;

use super::controls::{build_control_row, control_cmp, evaluate_dependencies};
use super::dialogs::{open_add_option, open_snippet_dialog};
use super::save::rescan;
use super::state::{App, ControlRow, InsertButton, MenuPage, Tx};

// ---------------------------------------------------------------------------
// scan and view
// ---------------------------------------------------------------------------

pub(super) fn open_flake(app: &Rc<App>, tx: &Tx, path: &str) {
    *app.base.borrow_mut() = path.to_string();
    app.plugin_registry.reset();
    *app.wallpaper_set.borrow_mut() = None;
    app.config.borrow_mut().remember(path);
    let _ = app.config.borrow().save();
    crate::nixd::warm(Some(path));
    rescan(app, tx);
}

pub(super) fn rebuild_view(app: &Rc<App>) {
    choose_current_page(app);
    rebuild_menu_list(app);
    rebuild_pages(app);
}

fn choose_current_page(app: &Rc<App>) {
    let menus = sorted_menus(app);
    let current = app.current_file.borrow().clone();
    if let Some(file) = select_menu_file(&menus, &current, app.page_pattern.as_ref()) {
        *app.current_file.borrow_mut() = file;
    }
}

fn visible_menus(app: &Rc<App>) -> Vec<crate::model::MenuModel> {
    let scan = app.scan.borrow();
    let Some(scan) = scan.as_ref() else {
        return Vec::new();
    };
    let host = app.current_host.borrow().clone();
    let menus: Vec<&crate::model::MenuModel> = if app.system_only.get() {
        scan.menus_for(&host)
    } else {
        scan.menus.iter().collect()
    };
    menus
        .into_iter()
        .filter(|menu| menu.has_menu)
        .filter(|menu| app.advanced.get() || !menu.spec.hidden)
        .cloned()
        .collect()
}

fn menu_cmp(a: &MenuModel, b: &MenuModel) -> Ordering {
    let ac = a.spec.category.as_deref().unwrap_or_default();
    let bc = b.spec.category.as_deref().unwrap_or_default();
    ac.cmp(bc)
        .then_with(|| a.order().cmp(&b.order()))
        .then_with(|| a.display_name().cmp(&b.display_name()))
}

fn sorted_menus(app: &Rc<App>) -> Vec<MenuModel> {
    let mut menus = visible_menus(app);
    menus.sort_by(menu_cmp);
    menus
}

fn select_menu_file(
    menus: &[MenuModel],
    current: &str,
    pattern: Option<&regex::Regex>,
) -> Option<String> {
    if menus.iter().any(|menu| menu.file == current) {
        return Some(current.to_string());
    }
    if let Some(pattern) = pattern {
        if let Some(menu) = menus
            .iter()
            .find(|menu| pattern.is_match(&menu.display_name()) || pattern.is_match(&menu.file))
        {
            return Some(menu.file.clone());
        }
    }
    menus.first().map(|menu| menu.file.clone())
}

fn menu_matches(menu: &crate::model::MenuModel, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if menu.display_name().to_lowercase().contains(query)
        || menu.file.to_lowercase().contains(query)
    {
        return true;
    }
    menu.records
        .iter()
        .any(|record| super::controls::record_matches_query(record, query))
}

const FALLBACK_MENU_ICON: &str = "preferences-system-symbolic";

pub(crate) fn pick_icon_name(
    has_icon: impl Fn(&str) -> bool,
    icon: Option<&str>,
) -> Option<String> {
    let name = icon?;
    if has_icon(name) {
        return Some(name.to_string());
    }
    if has_icon(FALLBACK_MENU_ICON) {
        return Some(FALLBACK_MENU_ICON.to_string());
    }
    None
}

fn resolve_menu_icon(icon: Option<&str>) -> Option<String> {
    let display = gtk::gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    pick_icon_name(|name| theme.has_icon(name), icon)
}

pub(super) fn rebuild_menu_list(app: &Rc<App>) {
    while let Some(child) = app.list.first_child() {
        app.list.remove(&child);
    }
    let query = app.search.text().to_lowercase();
    let menus = sorted_menus(app);
    let mut last_category: Option<String> = None;
    for menu in &menus {
        if !menu_matches(menu, &query) {
            continue;
        }
        let category = menu.spec.category.clone().unwrap_or_default();
        if last_category.as_deref() != Some(category.as_str()) {
            last_category = Some(category.clone());
            if !category.is_empty() {
                let header = gtk::Label::builder()
                    .label(&category)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .margin_top(6)
                    .css_classes(["group-header"])
                    .build();
                let row = gtk::ListBoxRow::builder()
                    .activatable(false)
                    .selectable(false)
                    .child(&header)
                    .build();
                app.list.append(&row);
            }
        }
        let name = menu.display_name();
        let label = gtk::Label::builder()
            .label(&name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&menu.file);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let Some(icon) = resolve_menu_icon(menu.spec.icon.as_deref()) {
            content.append(&gtk::Image::from_icon_name(icon.as_str()));
        }
        content.append(&label);
        row.set_child(Some(&content));
        app.list.append(&row);
    }

    let current = app.current_file.borrow().clone();
    select_file(app, &current);
}

fn select_file(app: &Rc<App>, file: &str) {
    if app.showing_inputs.get() {
        return;
    }
    let mut child = app.list.first_child();
    let mut first_selectable: Option<gtk::ListBoxRow> = None;
    while let Some(widget) = child {
        if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() {
            if !row.widget_name().is_empty() && first_selectable.is_none() {
                first_selectable = Some(row.clone());
            }
            if row.widget_name() == file {
                app.list.select_row(Some(row));
                return;
            }
        }
        child = widget.next_sibling();
    }
    if let Some(row) = first_selectable {
        app.list.select_row(Some(&row));
    }
}

fn rebuild_pages(app: &Rc<App>) {
    let old_pages = app.pages.take();
    for page in &old_pages {
        app.stack.remove(&page.root);
    }
    drop(old_pages);

    let menus = sorted_menus(app);
    let mut pages = Vec::new();
    for menu in &menus {
        let page = build_page(app, menu);
        app.stack.add_named(&page.root, Some(&menu.file));
        pages.push(page);
    }
    app.pages.replace(pages);

    if !app.showing_inputs.get() {
        let current = app.current_file.borrow().clone();
        if let Some(file) = select_menu_file(&menus, &current, None) {
            app.stack.set_visible_child_name(&file);
            *app.current_file.borrow_mut() = file.clone();
            select_file(app, &file);
        }
    }
    super::controls::apply_control_filter(app);
    super::enrich::query_current_page(app, &app.tx);
}

fn build_page(app: &Rc<App>, menu: &crate::model::MenuModel) -> MenuPage {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.set_hexpand(true);

    let heading = gtk::Label::builder()
        .label(menu.display_name())
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-2"])
        .build();
    content.append(&heading);
    if let Some(description) = &menu.spec.description {
        let subtitle = gtk::Label::builder()
            .label(description)
            .xalign(0.0)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .wrap(true)
            .css_classes(["dim-label"])
            .build();
        content.append(&subtitle);
    }

    let records: Rc<std::collections::HashMap<String, crate::model::Record>> = Rc::new(
        menu.records
            .iter()
            .map(|record| (record.path(), record.clone()))
            .collect(),
    );
    let mut ordinary = Vec::new();
    let mut advanced = Vec::new();
    let ranks = crate::model::group_ranks(&menu.records);
    for (src_index, record) in menu.records.iter().enumerate() {
        if record.spec.hidden && !app.advanced.get() {
            continue;
        }
        let row = build_control_row(app, record, src_index, records.clone());
        if record.spec.advanced {
            advanced.push(row);
        } else {
            ordinary.push(row);
        }
    }

    ordinary.sort_by(|a, b| control_cmp(a, b, &ranks));
    advanced.sort_by(|a, b| control_cmp(a, b, &ranks));

    let mut inserts = build_insert_buttons(app, menu);
    let mut headers = Vec::new();
    let mut group_inserts: Vec<(String, gtk::Button)> = Vec::new();
    add_grouped(
        &content,
        &ordinary,
        &mut inserts,
        false,
        &mut headers,
        &mut group_inserts,
    );
    if !advanced.is_empty() {
        let expander = gtk::Expander::new(Some("Advanced"));
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 12);
        inner.set_margin_top(6);
        let mut no_inserts = Vec::new();
        add_grouped(
            &inner,
            &advanced,
            &mut no_inserts,
            true,
            &mut headers,
            &mut group_inserts,
        );
        expander.set_child(Some(&inner));
        content.append(&expander);
    }

    inserts.sort_by(|a, b| a.group.cmp(&b.group).then_with(|| a.order.cmp(&b.order)));
    for insert in inserts {
        content.append(&insert.button);
    }

    let add = gtk::Button::with_label("Add option");
    add.add_css_class("pill");
    add.set_halign(gtk::Align::Start);
    {
        let app = app.clone();
        let file = menu.file.clone();
        let origin = menu
            .records
            .first()
            .map(|r| r.origin.clone())
            .unwrap_or_default();
        add.connect_clicked(move |_| {
            open_add_option(&app, &file, &origin);
        });
    }
    content.append(&add);

    content.append(&gtk::Box::new(gtk::Orientation::Vertical, 0));

    let scrolled = gtk::ScrolledWindow::builder()
        .child(&content)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_width(false)
        .vexpand(true)
        .hexpand(true)
        .build();

    let mut controls: Vec<Rc<ControlRow>> = ordinary;
    controls.extend(advanced);
    evaluate_dependencies(&controls);

    MenuPage {
        file: menu.file.clone(),
        controls,
        root: scrolled.upcast(),
        content,
        group_headers: headers,
        group_inserts,
    }
}

fn add_grouped(
    content: &gtk::Box,
    controls: &[Rc<ControlRow>],
    inserts: &mut Vec<InsertButton>,
    advanced: bool,
    headers: &mut Vec<(bool, String, gtk::Label)>,
    group_inserts: &mut Vec<(String, gtk::Button)>,
) {
    let mut last_group: Option<String> = None;
    for control in controls {
        let group = control.record.spec.group.clone().unwrap_or_default();
        if last_group.as_deref() != Some(group.as_str()) {
            if let Some(previous) = last_group.take() {
                flush_inserts(content, inserts, &previous, group_inserts);
            }
            if !group.is_empty() {
                let label = gtk::Label::builder()
                    .label(&group)
                    .xalign(0.0)
                    .margin_top(6)
                    .css_classes(["group-header"])
                    .build();
                content.append(&label);
                headers.push((advanced, group.clone(), label.clone()));
            }
            last_group = Some(group);
        }
        content.append(&control.row);
    }
    if let Some(previous) = last_group.take() {
        flush_inserts(content, inserts, &previous, group_inserts);
    }
}

/// Append the insert buttons assigned to `group`, in `order`, then drop them so
/// they are not placed again.
fn flush_inserts(
    content: &gtk::Box,
    inserts: &mut Vec<InsertButton>,
    group: &str,
    group_inserts: &mut Vec<(String, gtk::Button)>,
) {
    inserts.sort_by_key(|insert| insert.order);
    let mut index = 0;
    while index < inserts.len() {
        if inserts[index].group == group {
            let insert = inserts.remove(index);
            group_inserts.push((group.to_string(), insert.button.clone()));
            content.append(&insert.button);
        } else {
            index += 1;
        }
    }
}

/// Drop a leading `Add`/`add` from a snippet label so the button prefix does
/// not double it when the label already reads "Add ...".
pub(super) fn strip_add_prefix(label: &str) -> String {
    let trimmed = label.trim();
    let lower = trimmed.to_lowercase();
    if lower == "add" {
        String::new()
    } else if lower.starts_with("add ") {
        trimmed[4..].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Build the "Add <snippet>" button for every `@insert` marker, resolving its
/// target group from the marker, or from the record directly above it.
fn build_insert_buttons(app: &Rc<App>, menu: &MenuModel) -> Vec<InsertButton> {
    let mut out = Vec::new();
    for marker in &menu.inserts {
        let Some(snippet) = menu.snippets.iter().find(|s| s.name == marker.name) else {
            continue;
        };
        let label = marker
            .label
            .clone()
            .or_else(|| (!snippet.label.is_empty()).then(|| snippet.label.clone()))
            .unwrap_or_else(|| marker.name.clone());
        let label = strip_add_prefix(&label);
        let text = if label.is_empty() {
            "Add".to_string()
        } else {
            format!("Add {label}")
        };
        let button = gtk::Button::with_label(&text);
        button.add_css_class("pill");
        button.set_halign(gtk::Align::Start);
        {
            let app = app.clone();
            let file = menu.file.clone();
            let marker = marker.clone();
            let snippet = snippet.clone();
            button.connect_clicked(move |_| {
                open_snippet_dialog(&app, &file, &marker, &snippet);
            });
        }
        let group = marker
            .group
            .clone()
            .unwrap_or_else(|| preceding_group(menu, marker.offset));
        out.push(InsertButton {
            group,
            order: marker.order.unwrap_or(1000),
            button,
        });
    }
    out
}

/// The group of the record directly above `offset` in the file, or empty when
/// nothing precedes the marker.
fn preceding_group(menu: &MenuModel, offset: usize) -> String {
    menu.records
        .iter()
        .filter(|record| record.byte_start < offset)
        .max_by_key(|record| record.byte_start)
        .map(|record| record.spec.group.clone().unwrap_or_default())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::MenuSpec;

    fn menu(category: &str, order: i64, name: &str, file: &str) -> MenuModel {
        MenuModel {
            file: file.to_string(),
            spec: MenuSpec {
                category: Some(category.to_string()),
                order: Some(order),
                displayname: Some(name.to_string()),
                ..MenuSpec::default()
            },
            ..MenuModel::default()
        }
    }

    #[test]
    fn sorts_menus_by_category_then_order_then_name() {
        let mut menus = vec![
            menu("02-Core", 10, "Bar", "bar.nix"),
            menu("01-Users", 20, "User", "user.nix"),
            menu("01-Users", 10, "System", "system.nix"),
        ];
        menus.sort_by(menu_cmp);
        let names: Vec<_> = menus.iter().map(MenuModel::display_name).collect();
        assert_eq!(names, ["System", "User", "Bar"]);
    }

    #[test]
    fn selects_first_regex_match_in_sorted_order() {
        let menus = vec![
            menu("01-Users", 10, "User", "user.nix"),
            menu("02-Core", 10, "Bar", "bar.nix"),
            menu("02-Core", 20, "Dock", "dock.nix"),
        ];
        let pattern = regex::Regex::new("bar|dock").unwrap();
        assert_eq!(
            select_menu_file(&menus, "", Some(&pattern)).as_deref(),
            Some("bar.nix")
        );
    }

    #[test]
    fn falls_back_to_topmost_when_no_page_matches() {
        let menus = vec![
            menu("01-Users", 10, "User", "user.nix"),
            menu("02-Core", 10, "Bar", "bar.nix"),
        ];
        let pattern = regex::Regex::new("missing").unwrap();
        assert_eq!(
            select_menu_file(&menus, "", Some(&pattern)).as_deref(),
            Some("user.nix")
        );
        assert_eq!(
            select_menu_file(&menus, "", None).as_deref(),
            Some("user.nix")
        );
    }

    #[test]
    fn keeps_the_current_page_when_it_is_visible() {
        let menus = vec![
            menu("01-Users", 10, "User", "user.nix"),
            menu("02-Core", 10, "Bar", "bar.nix"),
        ];
        let pattern = regex::Regex::new("user").unwrap();
        assert_eq!(
            select_menu_file(&menus, "bar.nix", Some(&pattern)).as_deref(),
            Some("bar.nix")
        );
    }

    #[test]
    fn missing_menu_icon_falls_back_to_generic() {
        let only_fallback = |name: &str| name == "preferences-system-symbolic";
        assert_eq!(
            pick_icon_name(only_fallback, Some("preferences-system-displays-symbolic")).as_deref(),
            Some("preferences-system-symbolic")
        );
        let all_present = |_: &str| true;
        assert_eq!(
            pick_icon_name(all_present, Some("web-browser-symbolic")).as_deref(),
            Some("web-browser-symbolic")
        );
        assert_eq!(pick_icon_name(only_fallback, None), None);
    }
}
