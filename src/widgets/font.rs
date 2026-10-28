use super::editor::Editor;
use super::style_catalog;
use super::style_choice::{self, PackageBinding};
use crate::dsl::OptionSpec;
use gtk::pango;
use gtk::prelude::*;
use std::collections::HashSet;
use std::rc::Rc;

fn installed_families() -> HashSet<String> {
    pango::Context::default()
        .list_families()
        .into_iter()
        .map(|family| family.name().to_string())
        .collect()
}

fn styled_label(family: &str, size: i32, text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    let mut description = pango::FontDescription::new();
    description.set_family(family);
    description.set_absolute_size(size as f64 * pango::SCALE as f64);
    let attributes = pango::AttrList::new();
    attributes.insert(pango::AttrFontDesc::new(&description));
    label.set_attributes(Some(&attributes));
    label
}

fn thumbnail(family: String, installed: bool, preview: Option<&'static [u8]>) -> gtk::Widget {
    if installed {
        return styled_label(&family, 16, "Aa Bb Cc 0123").upcast();
    }
    if let Some(preview) = preview {
        return style_catalog::texture_widget(preview, 150, 40);
    }
    style_choice::placeholder(&family, 150, 40)
}

fn preview(entry: &'static style_catalog::StyleEntry, installed: bool) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.append(
        &gtk::Label::builder()
            .label(entry.label)
            .xalign(0.0)
            .css_classes(["title-4"])
            .build(),
    );
    let detail = format!(
        "{}  {}",
        if installed {
            "Installed"
        } else {
            "Not installed"
        },
        entry.package
    );
    root.append(
        &gtk::Label::builder()
            .label(detail)
            .xalign(0.0)
            .selectable(true)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    if installed {
        let sample = gtk::Box::new(gtk::Orientation::Vertical, 4);
        sample.append(&styled_label(entry.value, 26, "Aa Bb Cc Dd Ee 0123456789"));
        sample.append(&styled_label(
            entry.value,
            16,
            "The quick brown fox  {}[]()",
        ));
        root.append(&sample);
    } else if let Some(preview) = entry.preview {
        root.append(&style_catalog::texture_widget(preview, 420, 72));
    } else {
        root.append(&style_choice::placeholder(entry.value, 420, 72));
    }
    if entry.monospace {
        root.append(
            &gtk::Label::builder()
                .label("Monospace")
                .xalign(0.0)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
    }
    root.upcast()
}

pub(crate) fn build_font(
    value_text: &str,
    spec: &OptionSpec,
    package: Option<PackageBinding>,
) -> Editor {
    let installed = installed_families();
    let mut items = Vec::new();
    for entry in style_catalog::fonts() {
        if spec.monospace && !entry.monospace {
            continue;
        }
        let is_installed = installed.contains(entry.value);
        let family = entry.value.to_string();
        let preview_bytes = entry.preview;
        let thumbnail_family = family.clone();
        let search_extra = if entry.monospace { "monospace" } else { "" };
        items.push(style_choice::entry(
            entry.label,
            entry.value,
            Some(entry.package),
            is_installed,
            search_extra,
            Rc::new(move || thumbnail(thumbnail_family.clone(), is_installed, preview_bytes)),
            Rc::new(move || preview(entry, is_installed)),
        ));
    }
    style_choice::build(
        value_text,
        package,
        items,
        "Choose font",
        "Search installed and available fonts",
    )
}
