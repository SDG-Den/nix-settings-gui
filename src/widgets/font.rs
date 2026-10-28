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

fn preview(
    family: &str,
    label: &str,
    package: &str,
    monospace: bool,
    installed: bool,
    preview: Option<&'static [u8]>,
) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .css_classes(["title-4"])
            .build(),
    );
    root.append(
        &gtk::Label::builder()
            .label(format!(
                "{}  {package}",
                if installed {
                    "Installed"
                } else {
                    "Not installed"
                }
            ))
            .xalign(0.0)
            .selectable(true)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    if installed {
        let sample = gtk::Box::new(gtk::Orientation::Vertical, 4);
        sample.append(&styled_label(family, 26, "Aa Bb Cc Dd Ee 0123456789"));
        sample.append(&styled_label(family, 16, "The quick brown fox  {}[]()"));
        root.append(&sample);
    } else if let Some(preview) = preview {
        root.append(&style_catalog::texture_widget(preview, 420, 72));
    } else {
        root.append(&style_choice::placeholder(
            "Preview is available after the package is installed",
            420,
            72,
        ));
    }
    if monospace {
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
        let label = entry.label.to_string();
        let package = entry.package.to_string();
        let monospace = entry.monospace;
        let thumbnail = {
            let family = family.clone();
            Rc::new(move || thumbnail(family.clone(), is_installed, preview_bytes))
        };
        let preview = {
            let family = family.clone();
            let label = label.clone();
            let package = package.clone();
            Rc::new(move || {
                preview(
                    &family,
                    &label,
                    &package,
                    monospace,
                    is_installed,
                    preview_bytes,
                )
            })
        };
        items.push(style_choice::entry(
            label,
            family,
            Some(&package),
            is_installed,
            if monospace { "monospace" } else { "" },
            thumbnail,
            preview,
        ));
    }
    style_choice::build(
        value_text,
        package,
        items,
        "Search installed and available fonts",
    )
}
