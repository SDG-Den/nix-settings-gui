use gtk::prelude::*;

const TOGGLE_CLASS: &str = "details-toggle";

/// A collapsed description and origin row. The info button sits in the place
/// the helper text used to occupy and reveals the text when activated.
#[derive(Clone)]
pub(crate) struct InfoDetails {
    pub(crate) root: gtk::Box,
    pub(crate) button: gtk::ToggleButton,
    pub(crate) revealer: gtk::Revealer,
    description: gtk::Label,
    origin: gtk::Label,
}

impl InfoDetails {
    pub(crate) fn new(description: Option<&str>, origin: &str, expanded: bool) -> Self {
        let icon = gtk::Image::from_icon_name("dialog-information-symbolic");
        icon.set_pixel_size(16);
        let button = gtk::ToggleButton::new();
        button.add_css_class("flat");
        button.add_css_class(TOGGLE_CLASS);
        button.set_valign(gtk::Align::Start);
        button.set_child(Some(&icon));
        button.set_active(expanded);

        let description_label = gtk::Label::builder()
            .label(description.unwrap_or_default())
            .xalign(0.0)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .visible(description.is_some_and(|text| !text.is_empty()))
            .css_classes(["dim-label", "caption"])
            .width_chars(48)
            .build();

        let origin_text = if origin.is_empty() {
            String::new()
        } else {
            format!("imported from {origin}")
        };
        let origin_label = gtk::Label::builder()
            .label(origin_text)
            .xalign(0.0)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .wrap(true)
            .visible(!origin.is_empty())
            .css_classes(["dim-label", "caption"])
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        content.set_hexpand(true);
        content.append(&description_label);
        content.append(&origin_label);

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
        revealer.set_transition_duration(150);
        revealer.set_hexpand(true);
        revealer.set_child(Some(&content));

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        root.set_hexpand(true);
        root.set_valign(gtk::Align::Start);
        root.append(&button);
        root.append(&revealer);

        let revealer_for_toggle = revealer.clone();
        let description_for_toggle = description_label.clone();
        let origin_for_toggle = origin_label.clone();
        button.connect_toggled(move |button| {
            let has_details =
                !description_for_toggle.text().is_empty() || !origin_for_toggle.text().is_empty();
            revealer_for_toggle.set_reveal_child(has_details && button.is_active());
            button.set_tooltip_text(Some(toggle_tooltip(button.is_active())));
        });

        let details = Self {
            root,
            button,
            revealer,
            description: description_label,
            origin: origin_label,
        };
        details.sync();
        details
    }

    pub(crate) fn set_description(&self, text: &str) {
        self.description.set_text(text);
        self.sync();
    }

    fn sync(&self) {
        let has_description = !self.description.text().is_empty();
        let has_origin = !self.origin.text().is_empty();
        let has_details = has_description || has_origin;
        self.description.set_visible(has_description);
        self.origin.set_visible(has_origin);
        self.root.set_visible(has_details);
        self.revealer
            .set_reveal_child(has_details && self.button.is_active());
        self.button
            .set_tooltip_text(Some(toggle_tooltip(self.button.is_active())));
    }
}

fn toggle_tooltip(expanded: bool) -> &'static str {
    if expanded {
        "Hide details"
    } else {
        "Show details"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs a display; run with `cargo test -- --ignored`"]
    fn details_visibility_and_toggle() {
        if gtk::init().is_err() {
            eprintln!("skipping details runtime check: no display");
            return;
        }

        let hidden = InfoDetails::new(None, "", false);
        assert!(!hidden.root.is_visible());

        let origin_only = InfoDetails::new(None, "modules.nixos.audio", false);
        assert!(origin_only.root.is_visible());
        assert!(!origin_only.revealer.reveals_child());

        let expanded = InfoDetails::new(Some("Use the service"), "", true);
        assert!(expanded.button.is_active());
        assert!(expanded.revealer.reveals_child());

        let late = InfoDetails::new(None, "", true);
        assert!(!late.root.is_visible());
        late.set_description("Loaded from nixd");
        assert!(late.root.is_visible());
        assert!(late.revealer.reveals_child());

        let toggled = InfoDetails::new(Some("Use the service"), "", false);
        assert!(!toggled.revealer.reveals_child());
        toggled.button.set_active(true);
        assert!(toggled.revealer.reveals_child());
        toggled.button.set_active(false);
        assert!(!toggled.revealer.reveals_child());
    }
}
