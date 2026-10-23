use super::basic::build_entry;
use super::editor::Editor;
use super::nix::entry_text;
use crate::dsl::OptionSpec;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
mod schemes {
    include!(concat!(env!("OUT_DIR"), "/schemes.rs"));
}

#[cfg(test)]
#[path = "../../build_support.rs"]
mod build_support;

pub(crate) struct SchemeMeta {
    pub index: usize,
    pub name: String,
    pub category: Option<String>,
    pub variant: String,
}

fn prefix_matches(prefix: &str, all: &[&str]) -> usize {
    all.iter()
        .filter(|other| **other == prefix || other.starts_with(&format!("{prefix}-")))
        .count()
}

fn is_scheme(name: &str, all: &[&str]) -> bool {
    all.iter().any(|other| *other == name)
}

fn variant_of<'a>(name: &'a str, category: &str) -> &'a str {
    if name.len() == category.len() {
        ""
    } else {
        &name[category.len() + 1..]
    }
}

/// Split a scheme slug into (category, variant). A scheme joins the shortest
/// exact scheme prefix that covers more than one scheme, so `black-metal-venom`
/// groups under `black-metal` rather than `black`. With no exact prefix the
/// longest shared prefix wins, which keeps families such as `catppuccin` and
/// `da-one` grouped even though those base names are not schemes themselves.
pub(crate) fn split_variant<'a>(name: &'a str, all: &[&str]) -> (Option<&'a str>, &'a str) {
    let tokens: Vec<&str> = name.split('-').collect();
    let mut boundary = 0;
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            boundary += 1;
        }
        boundary += token.len();
        let prefix = &name[..boundary];
        if is_scheme(prefix, all) && prefix_matches(prefix, all) > 1 {
            return (Some(prefix), variant_of(name, prefix));
        }
    }
    let mut boundary = name.len();
    for index in (1..tokens.len()).rev() {
        boundary -= tokens[index].len() + 1;
        let prefix = &name[..boundary];
        if prefix_matches(prefix, all) > 1 {
            return (Some(prefix), variant_of(name, prefix));
        }
    }
    (None, name)
}

/// Turn a raw category slug into a display header: `everforest-dark` ->
/// `Everforest dark`, `catppuccin` -> `Catppuccin`.
pub(crate) fn pretty_category(category: &str) -> String {
    let mut words = category.split('-');
    let mut out = String::new();
    if let Some(first) = words.next() {
        let mut chars = first.chars();
        if let Some(head) = chars.next() {
            out.extend(head.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    for word in words {
        out.push(' ');
        out.push_str(word);
    }
    out
}

pub(crate) fn scheme_metas() -> Vec<SchemeMeta> {
    let names: Vec<&str> = schemes::SCHEMES.iter().map(|(name, _)| *name).collect();
    let mut metas: Vec<SchemeMeta> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let (category, variant) = split_variant(name, &names);
            SchemeMeta {
                index,
                name: name.to_string(),
                category: category.map(str::to_string),
                variant: variant.to_string(),
            }
        })
        .collect();
    collapse_nested_categories(&mut metas);
    metas
}

fn collapse_nested_categories(metas: &mut [SchemeMeta]) {
    let categories: Vec<String> = metas
        .iter()
        .filter_map(|meta| meta.category.clone())
        .collect();
    for meta in metas {
        let Some(category) = meta.category.clone() else {
            continue;
        };
        let parent = categories
            .iter()
            .filter(|other| category.starts_with(&format!("{other}-")))
            .min_by_key(|other| other.len());
        if let Some(parent) = parent {
            meta.variant = variant_of(&meta.name, parent).to_string();
            meta.category = Some(parent.clone());
        }
    }
}

fn row_label(meta: &SchemeMeta) -> &str {
    match &meta.category {
        Some(_) if meta.variant.is_empty() => "default",
        Some(_) => &meta.variant,
        None => &meta.name,
    }
}

struct ThemeGroup {
    header: gtk::ListBoxRow,
    button: gtk::Button,
    arrow: gtk::Image,
    rows: Vec<gtk::ListBoxRow>,
    gap: gtk::ListBoxRow,
    expanded: Cell<bool>,
}

struct ThemeSchemeRow {
    row: gtk::ListBoxRow,
    name: String,
    group: Option<usize>,
}

struct ThemeListRows {
    scheme_rows: Vec<ThemeSchemeRow>,
    groups: Vec<ThemeGroup>,
    query: RefCell<String>,
}

fn build_group_header(category: &str) -> (gtk::ListBoxRow, gtk::Button, gtk::Image) {
    let arrow = gtk::Image::from_icon_name("pan-down-symbolic");
    let label = gtk::Label::builder()
        .label(pretty_category(category))
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["group-header"])
        .build();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_margin_top(6);
    content.set_margin_bottom(2);
    content.set_margin_start(6);
    content.set_margin_end(6);
    content.append(&arrow);
    content.append(&label);
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("theme-group-toggle");
    button.set_hexpand(true);
    button.set_child(Some(&content));
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&button)
        .build();
    (row, button, arrow)
}

fn build_gap_row() -> gtk::ListBoxRow {
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_size_request(-1, 8);
    gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&spacer)
        .build()
}

fn set_group_expanded(group: &ThemeGroup, expanded: bool) {
    let icon = if expanded {
        "pan-down-symbolic"
    } else {
        "pan-end-symbolic"
    };
    group.arrow.set_icon_name(Some(icon));
    for row in &group.rows {
        row.set_visible(expanded);
    }
    group.gap.set_visible(expanded);
}

fn toggle_group(rows: &ThemeListRows, index: usize) {
    if !rows.query.borrow().trim().is_empty() {
        return;
    }
    let group = &rows.groups[index];
    let expanded = !group.expanded.get();
    group.expanded.set(expanded);
    set_group_expanded(group, expanded);
}

fn build_theme_list(list: &gtk::ListBox, metas: &[SchemeMeta], current: &str) -> Rc<ThemeListRows> {
    let mut scheme_rows = Vec::with_capacity(metas.len());
    let mut groups: Vec<ThemeGroup> = Vec::new();
    let mut current_group: Option<usize> = None;
    let mut selected_group: Option<usize> = None;
    let mut last_category: Option<&str> = None;
    let mut to_select: Option<gtk::ListBoxRow> = None;
    for meta in metas {
        if meta.category.as_deref() != last_category {
            if let Some(index) = current_group {
                let gap = groups[index].gap.clone();
                list.append(&gap);
            }
            last_category = meta.category.as_deref();
            if let Some(category) = &meta.category {
                let (header, button, arrow) = build_group_header(category);
                list.append(&header);
                groups.push(ThemeGroup {
                    header,
                    button,
                    arrow,
                    rows: Vec::new(),
                    gap: build_gap_row(),
                    expanded: Cell::new(true),
                });
                current_group = Some(groups.len() - 1);
            } else {
                current_group = None;
            }
        }
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.set_margin_top(4);
        content.set_margin_bottom(4);
        content.set_margin_start(6);
        content.set_margin_end(6);
        let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for ci in 0..16 {
            let swatch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            swatch.set_size_request(14, 14);
            swatch.add_css_class("theme-swatch");
            swatch.add_css_class(&format!("s{}-c{ci}", meta.index));
            swatches.append(&swatch);
        }
        content.append(&swatches);
        content.append(
            &gtk::Label::builder()
                .label(row_label(meta))
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&content));
        row.set_widget_name(&meta.name);
        row.set_tooltip_text(Some(&meta.name));
        list.append(&row);
        if let Some(index) = current_group {
            groups[index].rows.push(row.clone());
        }
        if meta.name == current {
            selected_group = current_group;
            to_select = Some(row.clone());
        }
        scheme_rows.push(ThemeSchemeRow {
            row,
            name: meta.name.clone(),
            group: current_group,
        });
    }
    if let Some(index) = current_group {
        let gap = groups[index].gap.clone();
        list.append(&gap);
    }
    for (index, group) in groups.iter().enumerate() {
        let expanded = selected_group == Some(index);
        group.expanded.set(expanded);
        set_group_expanded(group, expanded);
    }
    if let Some(row) = to_select {
        list.select_row(Some(&row));
    }
    let rows = Rc::new(ThemeListRows {
        scheme_rows,
        groups,
        query: RefCell::new(String::new()),
    });
    for (index, group) in rows.groups.iter().enumerate() {
        let rows = rows.clone();
        group
            .button
            .connect_clicked(move |_| toggle_group(&rows, index));
    }
    rows
}

fn apply_theme_filter(
    rows: &ThemeListRows,
    query: &str,
    list: &gtk::ListBox,
    scrolled: &gtk::ScrolledWindow,
) {
    let query = query.trim().to_lowercase();
    *rows.query.borrow_mut() = query.clone();
    let matches: Vec<bool> = rows
        .scheme_rows
        .iter()
        .map(|scheme| query.is_empty() || scheme.name.to_lowercase().contains(&query))
        .collect();
    let mut group_hits = vec![false; rows.groups.len()];
    for (scheme, matched) in rows.scheme_rows.iter().zip(&matches) {
        if *matched {
            if let Some(index) = scheme.group {
                group_hits[index] = true;
            }
        }
    }
    for (index, group) in rows.groups.iter().enumerate() {
        let expanded = if query.is_empty() {
            group.expanded.get()
        } else {
            group_hits[index]
        };
        group
            .header
            .set_visible(query.is_empty() || group_hits[index]);
        let icon = if expanded {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        };
        group.arrow.set_icon_name(Some(icon));
        group.gap.set_visible(expanded);
    }
    let mut first_hit: Option<&gtk::ListBoxRow> = None;
    for (scheme, matched) in rows.scheme_rows.iter().zip(&matches) {
        let visible = if query.is_empty() {
            scheme
                .group
                .map_or(true, |index| rows.groups[index].expanded.get())
        } else {
            *matched
        };
        scheme.row.remove_css_class("search-hit");
        scheme.row.set_visible(visible);
        if visible && !query.is_empty() && first_hit.is_none() {
            first_hit = Some(&scheme.row);
        }
    }
    if let Some(row) = first_hit {
        row.add_css_class("search-hit");
        if let Some(bounds) = row.compute_bounds(list) {
            scrolled
                .vadjustment()
                .set_value((bounds.y() as f64 - 12.0).max(0.0));
        }
    }
}

/// A scheme selector backed by the base16 palettes bundled at build time. Each
/// row shows the scheme's sixteen colors and its name; the written value is the
/// short scheme name. With no palettes bundled (plain `cargo test`, or a build
/// without TINTED_SCHEMES_DIR) it degrades to a labeled text entry.
pub(crate) fn build_theme(value_text: &str) -> Editor {
    if schemes::SCHEMES.is_empty() {
        return theme_text_fallback(value_text);
    }

    install_swatch_css();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let current = Rc::new(RefCell::new(entry_text(value_text)));

    let list = gtk::ListBox::new();
    list.add_css_class("theme-list");
    list.set_selection_mode(gtk::SelectionMode::Single);

    let metas = scheme_metas();
    let selected_name = current.borrow().clone();
    let rows = build_theme_list(&list, &metas, &selected_name);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search palettes")
        .hexpand(true)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(160)
        .vexpand(true)
        .child(&list)
        .build();

    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    left.set_hexpand(false);
    left.append(&search);
    left.append(&scrolled);

    let preview_host = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let initial = {
        let name = current.borrow().clone();
        schemes::SCHEMES
            .iter()
            .position(|(scheme, _)| *scheme == name.as_str())
            .unwrap_or(0)
    };
    show_preview(&preview_host, initial);

    let right = gtk::Box::new(gtk::Orientation::Vertical, 6);
    right.set_hexpand(true);
    right.append(&preview_host);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.set_hexpand(true);
    content.append(&left);
    content.append(&right);
    root.append(&content);

    {
        let current = current.clone();
        let preview_host = preview_host.clone();
        list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let name = row.widget_name().to_string();
            if name.is_empty() {
                return;
            }
            let Some(index) = schemes::SCHEMES
                .iter()
                .position(|(scheme, _)| *scheme == name)
            else {
                return;
            };
            *current.borrow_mut() = schemes::SCHEMES[index].0.to_string();
            show_preview(&preview_host, index);
        });
    }

    search.connect_search_changed({
        let (rows, list, scrolled) = (rows.clone(), list.clone(), scrolled.clone());
        move |entry| apply_theme_filter(&rows, &entry.text(), &list, &scrolled)
    });

    Editor::Theme {
        root,
        current,
        list,
        entry: None,
    }
}

const SAMPLE_CODE: &[&[(&str, usize)]] = &[
    &[("# base16 scheme preview", 3)],
    &[("{ config, pkgs, ... }:", 5)],
    &[("{", 5)],
    &[("  stylix = {", 5)],
    &[("    enable = ", 5), ("true", 9), (";", 5)],
    &[("    opacity = ", 5), ("0.95", 9), (";", 5)],
    &[
        ("    base16Scheme = ", 5),
        ("\"catppuccin-mocha\"", 11),
        (";", 5),
    ],
    &[
        ("    autoEnable = ", 5),
        ("mkForce", 13),
        (" ", 5),
        ("false", 9),
        (";", 5),
    ],
    &[("  };", 5)],
    &[
        ("  home.packages = ", 5),
        ("with", 14),
        (" pkgs; [ vim git ];", 5),
    ],
    &[("}", 5)],
];

fn code_line_markup(tokens: &[(&str, usize)], palette: &[&str; 16]) -> String {
    let mut out = String::new();
    for (text, index) in tokens {
        out.push_str(&format!(
            "<span foreground=\"{}\">{}</span>",
            palette[*index], text
        ));
    }
    out
}

fn raw_strip(index: usize) -> gtk::Box {
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    strip.add_css_class("theme-raw");
    strip.add_css_class(&format!("s{index}"));
    for ci in 0..16 {
        let swatch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        swatch.set_size_request(-1, 28);
        swatch.set_hexpand(true);
        swatch.add_css_class("theme-swatch");
        swatch.add_css_class(&format!("s{index}-c{ci}"));
        strip.append(&swatch);
    }
    strip
}

fn sample_ui(index: usize) -> gtk::Box {
    let ui = gtk::Box::new(gtk::Orientation::Vertical, 8);
    ui.set_size_request(320, -1);
    ui.add_css_class("theme-ui");
    ui.add_css_class(&format!("s{index}"));

    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.append(&gtk::Button::with_label("Apply"));
    let toggle = gtk::Switch::new();
    toggle.set_active(true);
    toggle.set_valign(gtk::Align::Center);
    line.append(&toggle);
    ui.append(&line);

    let entry = gtk::Entry::new();
    entry.set_text("Text entry");
    entry.set_editable(false);
    ui.append(&entry);

    ui.append(
        &gtk::Label::builder()
            .label("Default text")
            .xalign(0.0)
            .build(),
    );
    ui.append(
        &gtk::Label::builder()
            .label("Alternate text")
            .xalign(0.0)
            .css_classes(["theme-ui-dim"])
            .build(),
    );

    let rows = gtk::ListBox::new();
    rows.set_selection_mode(gtk::SelectionMode::Single);
    for name in ["First row", "Second row"] {
        let label = gtk::Label::builder()
            .label(name)
            .xalign(0.0)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&label));
        rows.append(&row);
    }
    if let Some(second) = rows.row_at_index(1) {
        rows.select_row(Some(&second));
    }
    ui.append(&rows);
    ui
}

fn sample_code(index: usize, palette: &[&str; 16]) -> gtk::Box {
    let code = gtk::Box::new(gtk::Orientation::Vertical, 2);
    code.add_css_class("theme-code");
    code.add_css_class(&format!("s{index}"));
    for line in SAMPLE_CODE {
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_markup(&code_line_markup(line, palette));
        code.append(&label);
    }
    code
}

fn section(title: &str, child: &gtk::Widget) -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["heading"])
            .build(),
    );
    col.append(child);
    col
}

fn build_preview(index: usize) -> gtk::Box {
    let (_, palette) = schemes::SCHEMES[index];
    let stack = gtk::Box::new(gtk::Orientation::Vertical, 12);
    stack.append(&section(
        "Raw palette",
        raw_strip(index).upcast_ref::<gtk::Widget>(),
    ));
    stack.append(&section(
        "Sample UI",
        sample_ui(index).upcast_ref::<gtk::Widget>(),
    ));
    stack.append(&section(
        "Sample code",
        sample_code(index, &palette).upcast_ref::<gtk::Widget>(),
    ));
    stack
}

/// Replace the preview column's single contents with scheme `index`.
fn show_preview(host: &gtk::Box, index: usize) {
    while let Some(child) = host.first_child() {
        host.remove(&child);
    }
    host.append(&build_preview(index));
}

pub(crate) fn theme_text_fallback(value_text: &str) -> Editor {
    let entry = build_entry(value_text, &OptionSpec::default(), false);
    let current = Rc::new(RefCell::new(entry.text().to_string()));
    let (entry_watch, current_watch) = (entry.clone(), current.clone());
    entry.connect_changed(move |_| {
        *current_watch.borrow_mut() = entry_watch.text().to_string();
    });
    let warning = gtk::Label::new(Some(
        "No schemes bundled: set TINTED_SCHEMES_DIR and rebuild",
    ));
    warning.set_xalign(0.0);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    root.append(&warning);
    root.append(&entry);
    Editor::Theme {
        root,
        current,
        list: gtk::ListBox::new(),
        entry: Some(entry),
    }
}

/// Install the one-time stylesheet that paints each palette swatch.
pub(crate) fn install_swatch_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mut css = static_theme_css();
        for (si, (_, palette)) in schemes::SCHEMES.iter().enumerate() {
            css.push_str(&scheme_css(si, palette));
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_data(&css);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

fn static_theme_css() -> String {
    let mut css = String::new();
    css.push_str(
        ".theme-group-toggle { padding: 0; background-color: transparent; background-image: none; box-shadow: none; }\n",
    );
    css.push_str(".theme-raw { padding: 6px; border-radius: 6px; }\n");
    css.push_str(".theme-raw .theme-swatch { border-radius: 3px; }\n");
    css.push_str(".theme-ui { padding: 10px; border-radius: 6px; }\n");
    css.push_str(
        ".theme-code { padding: 10px; border-radius: 6px; font-family: monospace; font-size: 13px; }\n",
    );
    css
}

fn scheme_css(index: usize, palette: &[&str; 16]) -> String {
    let mut css = String::new();
    for (ci, color) in palette.iter().enumerate() {
        css.push_str(&format!(
            ".theme-swatch.s{index}-c{ci} {{ background-color: {color}; }}\n"
        ));
    }
    let (c00, c01, c02, c03, c04, c05) = (
        palette[0], palette[1], palette[2], palette[3], palette[4], palette[5],
    );
    css.push_str(&format!(
        ".theme-raw.s{index} {{ background-color: {c01}; border: 1px solid {c02}; }}\n"
    ));
    css.push_str(&format!(".theme-ui.s{index} {{ background-color: {c00}; color: {c05}; border: 1px solid {c02}; }}\n"));
    css.push_str(&format!(
        ".theme-ui.s{index} button {{ background-color: {c02}; background-image: none; box-shadow: none; color: {c05}; border-color: {c03}; }}\n"
    ));
    css.push_str(&format!(
        ".theme-ui.s{index} entry {{ background-color: {c01}; background-image: none; box-shadow: none; color: {c05}; caret-color: {c05}; }}\n"
    ));
    css.push_str(&format!(
        ".theme-ui.s{index} entry selection {{ background-color: {c02}; color: {c05}; }}\n"
    ));
    css.push_str(&format!(
        ".theme-ui.s{index} switch {{ background-color: {c01}; background-image: none; box-shadow: none; }}\n"
    ));
    css.push_str(&format!(
        ".theme-ui.s{index} switch:checked {{ background-color: {c02}; background-image: none; box-shadow: none; }}\n"
    ));
    css.push_str(&format!(
        ".theme-ui.s{index} switch slider {{ background-color: {c05}; box-shadow: none; }}\n"
    ));
    css.push_str(&format!(
        ".theme-ui.s{index} list {{ background-color: {c01}; }}\n"
    ));
    css.push_str(&format!(
        ".theme-ui.s{index} list row {{ background-color: {c01}; color: {c05}; }}\n"
    ));
    css.push_str(&format!(
        ".theme-ui.s{index} list row:selected {{ background-color: {c02}; color: {c05}; }}\n"
    ));
    css.push_str(&format!(
        ".theme-ui.s{index} label.theme-ui-dim {{ color: {c04}; }}\n"
    ));
    css.push_str(&format!(".theme-code.s{index} {{ background-color: {c00}; color: {c05}; border: 1px solid {c02}; }}\n"));
    css
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_schemes_populated_when_env_provided() {
        if option_env!("TINTED_SCHEMES_DIR").is_none() {
            return;
        }
        assert!(
            schemes::SCHEMES.len() > 200,
            "expected base16 schemes, got {}",
            schemes::SCHEMES.len()
        );
    }

    #[test]
    fn parses_bundled_palette_stub() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/palettes/base16-stub.yaml"
        );
        let text = std::fs::read_to_string(path).unwrap();
        let (name, colors) = build_support::parse_scheme_text(&text, "base16-stub").unwrap();
        assert_eq!(name, "stub");
        assert_eq!(colors[0], "#111111");
        assert_eq!(colors[15], "#552200");
    }

    #[test]
    fn variant_split_groups_under_meaningful_prefixes() {
        let all = [
            "aroz-con-dulce",
            "aroz-con-dulce-dark",
            "ascendancy",
            "ashes",
            "atelier-cave",
            "atelier-cave-light",
            "atelier-dune",
            "atelier-dune-light",
            "black-metal",
            "black-metal-bathory",
            "black-metal-venom",
            "catppuccin-latte",
            "catppuccin-mocha",
            "da-one-dark",
            "da-one-gruvbox",
            "everforest",
            "everforest-dark-hard",
            "everforest-dark-medium",
            "everforest-light-hard",
            "github",
            "github-dark",
            "github-dark-dimmed",
            "github-dark-high-contrast",
            "github-light",
            "nord",
            "nord-light",
        ];
        assert_eq!(split_variant("nord", &all), (Some("nord"), ""));
        assert_eq!(
            split_variant("catppuccin-mocha", &all),
            (Some("catppuccin"), "mocha")
        );
        assert_eq!(
            split_variant("everforest-dark-hard", &all),
            (Some("everforest"), "dark-hard")
        );
        assert_eq!(split_variant("da-one-dark", &all), (Some("da-one"), "dark"));
        assert_eq!(
            split_variant("github-dark-high-contrast", &all),
            (Some("github"), "dark-high-contrast")
        );
        assert_eq!(
            split_variant("github-light", &all),
            (Some("github"), "light")
        );
        assert_eq!(
            split_variant("black-metal", &all),
            (Some("black-metal"), "")
        );
        assert_eq!(
            split_variant("black-metal-venom", &all),
            (Some("black-metal"), "venom")
        );
        assert_eq!(
            split_variant("atelier-cave", &all),
            (Some("atelier-cave"), "")
        );
        assert_eq!(
            split_variant("atelier-cave-light", &all),
            (Some("atelier-cave"), "light")
        );
        assert_eq!(
            split_variant("aroz-con-dulce", &all),
            (Some("aroz-con-dulce"), "")
        );
        assert_eq!(
            split_variant("aroz-con-dulce-dark", &all),
            (Some("aroz-con-dulce"), "dark")
        );
        assert_eq!(split_variant("ascendancy", &all), (None, "ascendancy"));
        assert_eq!(split_variant("ashes", &all), (None, "ashes"));
    }

    #[test]
    fn grouped_rows_show_only_the_variant() {
        let metas = vec![
            SchemeMeta {
                index: 0,
                name: "atelier-cave".to_string(),
                category: Some("atelier-cave".to_string()),
                variant: String::new(),
            },
            SchemeMeta {
                index: 1,
                name: "atelier-cave-light".to_string(),
                category: Some("atelier-cave".to_string()),
                variant: "light".to_string(),
            },
            SchemeMeta {
                index: 2,
                name: "ashes".to_string(),
                category: None,
                variant: "ashes".to_string(),
            },
        ];
        assert_eq!(row_label(&metas[0]), "default");
        assert_eq!(row_label(&metas[1]), "light");
        assert_eq!(row_label(&metas[2]), "ashes");
    }

    #[test]
    fn nested_categories_collapse_into_the_shortest_parent() {
        let mut metas = vec![
            SchemeMeta {
                index: 0,
                name: "equilibrium-dark".to_string(),
                category: Some("equilibrium".to_string()),
                variant: "dark".to_string(),
            },
            SchemeMeta {
                index: 1,
                name: "equilibrium-gray-dark".to_string(),
                category: Some("equilibrium-gray".to_string()),
                variant: "dark".to_string(),
            },
            SchemeMeta {
                index: 2,
                name: "equilibrium-gray-light".to_string(),
                category: Some("equilibrium-gray".to_string()),
                variant: "light".to_string(),
            },
            SchemeMeta {
                index: 3,
                name: "equilibrium-light".to_string(),
                category: Some("equilibrium".to_string()),
                variant: "light".to_string(),
            },
        ];
        collapse_nested_categories(&mut metas);
        for meta in &metas {
            assert_eq!(meta.category.as_deref(), Some("equilibrium"));
        }
        assert_eq!(metas[1].variant, "gray-dark");
        assert_eq!(metas[2].variant, "gray-light");
    }

    #[test]
    fn category_headers_are_prettified() {
        assert_eq!(pretty_category("catppuccin"), "Catppuccin");
        assert_eq!(pretty_category("everforest-dark"), "Everforest dark");
        assert_eq!(pretty_category("github-dark"), "Github dark");
    }

    #[test]
    fn scheme_metas_are_sorted_and_prefix_grouped() {
        if option_env!("TINTED_SCHEMES_DIR").is_none() {
            return;
        }
        let metas = scheme_metas();
        assert_eq!(metas.len(), schemes::SCHEMES.len());
        for meta in &metas {
            if let Some(category) = &meta.category {
                assert!(
                    meta.name == *category || meta.name.starts_with(&format!("{category}-")),
                    "{:?} is not inside its category {:?}",
                    meta.name,
                    category
                );
            }
        }
        for (name, category) in [
            ("black-metal", "black-metal"),
            ("atelier-cave", "atelier-cave"),
            ("everforest", "everforest"),
            ("github", "github"),
        ] {
            if let Some(meta) = metas.iter().find(|meta| meta.name == name) {
                assert_eq!(meta.category.as_deref(), Some(category), "{name}");
            }
        }
    }

    fn stub_palette() -> [&'static str; 16] {
        [
            "#111111", "#222222", "#333333", "#444444", "#555555", "#666666", "#777777", "#888888",
            "#990000", "#aa5500", "#aaaa00", "#00aa00", "#00aaaa", "#0000aa", "#aa00aa", "#552200",
        ]
    }

    #[test]
    fn scheme_css_covers_swatches_ui_and_code() {
        let css = scheme_css(3, &stub_palette());
        assert!(css.contains(".theme-swatch.s3-c0 "));
        assert!(css.contains(".theme-swatch.s3-c15 "));
        assert!(css.contains(".theme-ui.s3 {"));
        assert!(css.contains(".theme-ui.s3 button "));
        assert!(css.contains(".theme-code.s3 {"));
        assert!(css.contains("#aa00aa"));
    }

    #[test]
    fn code_markup_uses_syntax_roles() {
        let markup = code_line_markup(&[("true", 9), (";", 5)], &stub_palette());
        assert_eq!(
            markup,
            "<span foreground=\"#aa5500\">true</span><span foreground=\"#666666\">;</span>"
        );
    }

    #[test]
    #[ignore = "needs a display; run with `cargo test -- --ignored`"]
    fn theme_filter_hides_rows_and_headers() {
        if gtk::init().is_err() {
            eprintln!("skipping theme filter runtime check: no display");
            return;
        }
        let metas = vec![
            SchemeMeta {
                index: 0,
                name: "nord".to_string(),
                category: None,
                variant: "nord".to_string(),
            },
            SchemeMeta {
                index: 1,
                name: "catppuccin-latte".to_string(),
                category: Some("catppuccin".to_string()),
                variant: "latte".to_string(),
            },
            SchemeMeta {
                index: 2,
                name: "catppuccin-mocha".to_string(),
                category: Some("catppuccin".to_string()),
                variant: "mocha".to_string(),
            },
        ];
        let list = gtk::ListBox::new();
        let rows = build_theme_list(&list, &metas, "nord");
        let scrolled = gtk::ScrolledWindow::new();

        apply_theme_filter(&rows, "mocha", &list, &scrolled);
        let visible: Vec<(&str, bool)> = rows
            .scheme_rows
            .iter()
            .map(|scheme| (scheme.name.as_str(), scheme.row.is_visible()))
            .collect();
        assert_eq!(
            visible,
            vec![
                ("nord", false),
                ("catppuccin-latte", false),
                ("catppuccin-mocha", true)
            ]
        );
        assert!(rows.groups[0].header.is_visible());

        apply_theme_filter(&rows, "no-such-scheme", &list, &scrolled);
        assert!(rows
            .scheme_rows
            .iter()
            .all(|scheme| !scheme.row.is_visible()));
        assert!(!rows.groups[0].header.is_visible());
        assert_eq!(rows.scheme_rows[0].name.as_str(), "nord");
    }
}
