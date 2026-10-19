use super::basic::build_entry;
use super::editor::Editor;
use super::nix::entry_text;
use crate::dsl::OptionSpec;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
mod schemes {
    include!(concat!(env!("OUT_DIR"), "/schemes.rs"));
}

#[cfg(test)]
#[path = "../../build_support.rs"]
mod build_support;
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

    for (index, (name, palette)) in schemes::SCHEMES.iter().enumerate() {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.set_margin_top(4);
        content.set_margin_bottom(4);
        content.set_margin_start(6);
        content.set_margin_end(6);
        let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for (ci, _) in palette.iter().enumerate() {
            let swatch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            swatch.set_size_request(14, 14);
            swatch.add_css_class("theme-swatch");
            swatch.add_css_class(&format!("s{index}-c{ci}"));
            swatches.append(&swatch);
        }
        content.append(&swatches);
        content.append(&gtk::Label::new(Some(name)));
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&content));
        list.append(&row);
        if *current.borrow() == *name {
            list.select_row(Some(&row));
        }
    }

    {
        let current = current.clone();
        list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                if let Some((name, _)) = schemes::SCHEMES.get(row.index() as usize) {
                    *current.borrow_mut() = (*name).to_string();
                }
            }
        });
    }

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(160)
        .child(&list)
        .build();
    scrolled.set_hexpand(true);
    root.append(&scrolled);
    Editor::Theme {
        root,
        current,
        list,
        entry: None,
    }
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
        let mut css = String::new();
        for (si, (_, palette)) in schemes::SCHEMES.iter().enumerate() {
            for (ci, color) in palette.iter().enumerate() {
                css.push_str(&format!(
                    ".theme-swatch.s{si}-c{ci} {{ background-color: {color}; }}\n"
                ));
            }
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
}
