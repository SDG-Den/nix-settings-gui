use super::editor::Editor;
use super::style_catalog;
use super::style_choice::{self, PackageBinding};
use super::theme_catalog::{self, ThemeRoot};
use super::xcursor;
use crate::dsl::OptionSpec;
use gtk::{gdk, glib, prelude::*};
use std::fs;
use std::path::Path;
use std::rc::Rc;

const SAMPLE_CURSORS: &[&[&str]] = &[
    &["default", "left_ptr"],
    &["pointer", "hand1"],
    &["text", "xterm"],
    &["wait", "watch"],
    &["crosshair"],
    &["move", "fleur"],
    &["not-allowed", "crossed_circle"],
    &["ew-resize"],
    &["ns-resize"],
];

fn cursor_widget(path: &Path, size: i32) -> Option<gtk::Widget> {
    let data = fs::read(path).ok()?;
    let cursor = xcursor::decode(&data)?;
    let bytes = glib::Bytes::from_owned(cursor.rgba);
    let texture = gdk::MemoryTexture::new(
        cursor.width as i32,
        cursor.height as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        cursor.width as usize * 4,
    );
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_size_request(size, size);
    Some(picture.upcast())
}

fn sample_path(theme: &ThemeRoot, names: &[&str]) -> Option<std::path::PathBuf> {
    let dir = theme.path.join("cursors");
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

fn cursor_grid(theme: &ThemeRoot, size: i32) -> gtk::Widget {
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_column_spacing(8);
    flow.set_row_spacing(8);
    flow.set_max_children_per_line(5);
    flow.set_valign(gtk::Align::Start);
    for names in SAMPLE_CURSORS {
        let Some(path) = sample_path(theme, names) else {
            continue;
        };
        if let Some(widget) = cursor_widget(&path, size) {
            flow.append(&widget);
        }
    }
    if flow.first_child().is_none() {
        flow.append(&style_choice::placeholder(&theme.label, 200, size));
    }
    flow.upcast()
}

fn asset_grid(previews: &'static [&'static [u8]], size: i32) -> gtk::Widget {
    if previews.is_empty() {
        return style_choice::placeholder("No preview available", 200, size);
    }
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_column_spacing(8);
    flow.set_row_spacing(8);
    flow.set_max_children_per_line(5);
    for bytes in previews {
        flow.append(&style_catalog::texture_widget(bytes, size, size));
    }
    flow.upcast()
}

fn thumbnail(theme: Option<ThemeRoot>, entry: &'static style_catalog::StyleEntry) -> gtk::Widget {
    if let Some(theme) = theme {
        if let Some(path) = sample_path(&theme, &["default", "left_ptr"]) {
            if let Some(widget) = cursor_widget(&path, 28) {
                return widget;
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
        root.append(&cursor_grid(&theme, 40));
    } else {
        root.append(&asset_grid(entry.previews, 40));
    }
    root.upcast()
}

pub(crate) fn build_cursor_theme(
    value_text: &str,
    _spec: &OptionSpec,
    package: Option<PackageBinding>,
) -> Editor {
    let mut items = Vec::new();
    for entry in style_catalog::cursor_themes() {
        let theme = theme_catalog::find_cursor(entry.value);
        let installed = theme.is_some();
        let thumbnail_theme = theme.clone();
        items.push(style_choice::entry(
            entry.label,
            entry.value,
            Some(entry.package),
            installed,
            "",
            Rc::new(move || thumbnail(thumbnail_theme.clone(), entry)),
            Rc::new(move || preview(entry, theme.clone(), installed)),
        ));
    }
    style_choice::build(
        value_text,
        package,
        items,
        "Choose cursor theme",
        "Search installed and available cursor themes",
    )
}
