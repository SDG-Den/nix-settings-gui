use super::context::BuildContext;
use super::editor::Editor;
use super::style_catalog;
use super::style_choice::{self, PackageBinding};
use crate::dsl::OptionSpec;
use gtk::pango;
use gtk::prelude::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

fn installed_families() -> HashMap<String, bool> {
    pango::Context::default()
        .list_families()
        .into_iter()
        .map(|family| (family.name().to_string(), family.is_monospace()))
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
    package: Option<&str>,
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
    let detail = match package {
        Some(package) => format!(
            "{}  {package}",
            if installed {
                "Installed"
            } else {
                "Not installed"
            }
        ),
        None => "Installed outside the catalog".to_string(),
    };
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
    ctx: &BuildContext,
) -> Editor {
    let installed = installed_families();
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    if let Some(fonts) = &ctx.style_fonts {
        for font in fonts.iter() {
            if spec.monospace && !font.monospace {
                continue;
            }
            if !seen.insert(font.family.clone()) {
                continue;
            }
            let is_installed = installed.contains_key(&font.family);
            let preview_bytes = style_catalog::fonts()
                .iter()
                .find(|entry| entry.value == font.family)
                .and_then(|entry| entry.preview);
            let family = font.family.clone();
            let label = font.label.clone();
            let package = font.package.clone();
            let monospace = font.monospace;
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
                        Some(&package),
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
    }

    for entry in style_catalog::fonts() {
        if spec.monospace && !entry.monospace {
            continue;
        }
        if !seen.insert(entry.value.to_string()) {
            continue;
        }
        let is_installed = installed.contains_key(entry.value);
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
                    Some(&package),
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

    let mut extras: Vec<(String, bool)> = installed
        .iter()
        .filter(|(family, monospace)| !seen.contains(*family) && (!spec.monospace || **monospace))
        .map(|(family, monospace)| (family.clone(), *monospace))
        .collect();
    extras.sort_by_key(|(family, _)| family.to_lowercase());
    for (family, monospace) in extras {
        let label = family.clone();
        items.push(style_choice::entry(
            label.clone(),
            family.clone(),
            None,
            true,
            if monospace { "monospace" } else { "" },
            Rc::new({
                let family = family.clone();
                move || thumbnail(family.clone(), true, None)
            }),
            Rc::new(move || preview(&family, &label, None, monospace, true, None)),
        ));
    }

    style_choice::build(
        value_text,
        package,
        items,
        "Search installed and available fonts",
    )
}
