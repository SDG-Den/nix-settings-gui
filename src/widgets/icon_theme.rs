use super::editor::Editor;
use super::style_catalog;
use super::style_choice::{self, PackageBinding};
use super::theme_catalog::{self, ThemeRoot};
use crate::dsl::OptionSpec;
use gtk::prelude::*;
use std::collections::HashSet;
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

fn thumbnail(theme: Option<ThemeRoot>, previews: &'static [&'static [u8]]) -> gtk::Widget {
    if let Some(theme) = theme {
        for name in ["folder", "system-file-manager", "computer"] {
            if let Some(icon) = theme_icon(&theme, name, 24) {
                return icon;
            }
        }
    }
    style_catalog::preview_widget(previews, 120, 40)
}

fn preview(
    label: &str,
    package: &str,
    theme: Option<ThemeRoot>,
    installed: bool,
    previews: &'static [&'static [u8]],
) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .css_classes(["title-4"])
            .build(),
    );
    let detail = if package.is_empty() {
        "Installed theme".to_string()
    } else {
        format!(
            "{}  {package}",
            if installed {
                "Installed"
            } else {
                "Not installed"
            }
        )
    };
    root.append(
        &gtk::Label::builder()
            .label(detail)
            .xalign(0.0)
            .selectable(true)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    if let Some(theme) = theme {
        root.append(&icon_grid(&theme, 32, 4));
    } else {
        root.append(&asset_grid(previews, 40, 4));
    }
    root.upcast()
}

pub(crate) fn build_icon_theme(
    value_text: &str,
    spec: &OptionSpec,
    package: Option<PackageBinding>,
) -> Editor {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for entry in style_catalog::icon_themes() {
        if let Some(variant) = spec.variant.as_deref() {
            if entry.variant != variant {
                continue;
            }
        }
        seen.insert(entry.value.to_string());
        let theme = theme_catalog::find(entry.value);
        let installed = theme.is_some();
        let label = entry.label.to_string();
        let package = entry.package.to_string();
        let previews = entry.previews;
        let thumbnail_theme = theme.clone();
        items.push(style_choice::entry(
            label.clone(),
            entry.value,
            Some(entry.package),
            installed,
            entry.variant,
            Rc::new(move || thumbnail(thumbnail_theme.clone(), previews)),
            Rc::new(move || preview(&label, &package, theme.clone(), installed, previews)),
        ));
    }
    if spec.variant.is_none() {
        for theme in theme_catalog::roots() {
            if !seen.insert(theme.name.clone()) {
                continue;
            }
            let label = theme.label.clone();
            let thumbnail_theme = theme.clone();
            let preview_theme = theme.clone();
            items.push(style_choice::entry(
                label.clone(),
                theme.name.clone(),
                None,
                true,
                "installed",
                Rc::new(move || thumbnail(Some(thumbnail_theme.clone()), &[])),
                Rc::new(move || preview(&label, "", Some(preview_theme.clone()), true, &[])),
            ));
        }
    }
    style_choice::build(
        value_text,
        package,
        items,
        "Search installed and available icon themes",
    )
}
