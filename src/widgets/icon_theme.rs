use super::editor::Editor;
use super::style_catalog;
use super::style_choice::{self, PackageBinding};
use super::theme_catalog::{self, ThemeRoot};
use crate::dsl::OptionSpec;
use gtk::prelude::*;
use std::rc::Rc;

const SAMPLE_ICONS: &[&str] = &[
    "folder",
    "folder-symbolic",
    "internet-web-browser",
    "system-file-manager",
    "edit-copy",
    "dialog-information",
    "computer",
    "utilities-terminal",
    "view-refresh",
    "user-home",
];

fn theme_icon(theme: &ThemeRoot, name: &str, size: i32) -> Option<gtk::Widget> {
    let icon_theme = gtk::IconTheme::new();
    if let Some(display) = gtk::gdk::Display::default() {
        icon_theme.set_display(Some(&display));
    }
    icon_theme.set_search_path(&[theme.parent.as_path()]);
    icon_theme.set_theme_name(Some(&theme.name));
    if !icon_theme.has_icon(name) {
        return None;
    }
    let paintable = icon_theme.lookup_icon(
        name,
        &["image-x-generic"],
        size,
        1,
        gtk::TextDirection::None,
        gtk::IconLookupFlags::empty(),
    );
    let image = gtk::Image::from_paintable(Some(&paintable));
    image.set_pixel_size(size);
    Some(image.upcast())
}

fn icon_grid(theme: &ThemeRoot, size: i32, columns: u32) -> gtk::Widget {
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_column_spacing(8);
    flow.set_row_spacing(8);
    flow.set_max_children_per_line(columns);
    flow.set_valign(gtk::Align::Start);
    for name in SAMPLE_ICONS {
        if let Some(icon) = theme_icon(theme, name, size) {
            flow.append(&icon);
        }
    }
    if flow.first_child().is_none() {
        flow.append(&style_choice::placeholder(
            &theme.label,
            40 * columns as i32,
            size,
        ));
    }
    flow.upcast()
}

fn asset_grid(previews: &'static [&'static [u8]], size: i32, columns: u32) -> gtk::Widget {
    if previews.is_empty() {
        return style_choice::placeholder("No preview available", 180, size);
    }
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_column_spacing(8);
    flow.set_row_spacing(8);
    flow.set_max_children_per_line(columns);
    for bytes in previews {
        flow.append(&style_catalog::texture_widget(bytes, size, size));
    }
    flow.upcast()
}

fn thumbnail(theme: Option<ThemeRoot>, entry: &'static style_catalog::StyleEntry) -> gtk::Widget {
    if let Some(theme) = theme {
        for name in ["folder", "system-file-manager", "computer"] {
            if let Some(icon) = theme_icon(&theme, name, 24) {
                return icon;
            }
        }
    }
    style_catalog::preview_widget(entry.previews, 120, 40)
}

fn preview(
    entry: &'static style_catalog::StyleEntry,
    theme: Option<ThemeRoot>,
    installed: bool,
) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.append(
        &gtk::Label::builder()
            .label(entry.label)
            .xalign(0.0)
            .css_classes(["title-4"])
            .build(),
    );
    root.append(
        &gtk::Label::builder()
            .label(format!(
                "{}  {}",
                if installed {
                    "Installed"
                } else {
                    "Not installed"
                },
                entry.package
            ))
            .xalign(0.0)
            .selectable(true)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    if let Some(theme) = theme {
        root.append(&icon_grid(&theme, 32, 4));
    } else {
        root.append(&asset_grid(entry.previews, 40, 4));
    }
    root.upcast()
}

pub(crate) fn build_icon_theme(
    value_text: &str,
    spec: &OptionSpec,
    package: Option<PackageBinding>,
) -> Editor {
    let mut items = Vec::new();
    for entry in style_catalog::icon_themes() {
        if let Some(variant) = spec.variant.as_deref() {
            if entry.variant != variant {
                continue;
            }
        }
        let theme = theme_catalog::find(entry.value);
        let installed = theme.is_some();
        let thumbnail_theme = theme.clone();
        items.push(style_choice::entry(
            entry.label,
            entry.value,
            Some(entry.package),
            installed,
            entry.variant,
            Rc::new(move || thumbnail(thumbnail_theme.clone(), entry)),
            Rc::new(move || preview(entry, theme.clone(), installed)),
        ));
    }
    style_choice::build(
        value_text,
        package,
        items,
        "Choose icon theme",
        "Search installed and available icon themes",
    )
}
