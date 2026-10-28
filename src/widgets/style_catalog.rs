use gtk::{gdk, glib, prelude::*};

pub(crate) struct StyleEntry {
    pub label: &'static str,
    pub value: &'static str,
    pub package: &'static str,
    pub preview: Option<&'static [u8]>,
    pub previews: &'static [&'static [u8]],
    pub monospace: bool,
    pub variant: &'static str,
}

mod data {
    include!(concat!(env!("OUT_DIR"), "/style_catalog.rs"));
}

pub(crate) fn fonts() -> &'static [StyleEntry] {
    data::FONTS
}

pub(crate) fn icon_themes() -> &'static [StyleEntry] {
    data::ICON_THEMES
}

pub(crate) fn cursor_themes() -> &'static [StyleEntry] {
    data::CURSOR_THEMES
}

pub(crate) fn texture(bytes: &'static [u8]) -> Option<gdk::Texture> {
    gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)).ok()
}

pub(crate) fn texture_widget(bytes: &'static [u8], width: i32, height: i32) -> gtk::Widget {
    if let Some(texture) = texture(bytes) {
        let picture = gtk::Picture::for_paintable(&texture);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        picture.set_size_request(width, height);
        return picture.upcast();
    }
    let icon = gtk::Image::from_icon_name("image-missing-symbolic");
    icon.set_pixel_size(width.min(height));
    icon.upcast()
}

pub(crate) fn preview_widget(
    paths: &'static [&'static [u8]],
    width: i32,
    height: i32,
) -> gtk::Widget {
    if let Some(bytes) = paths.first() {
        return texture_widget(bytes, width, height);
    }
    let icon = gtk::Image::from_icon_name("image-missing-symbolic");
    icon.set_pixel_size(width.min(height));
    icon.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_catalog_entries_use_nix_packages() {
        for entry in fonts().iter().chain(icon_themes()).chain(cursor_themes()) {
            assert!(!entry.label.is_empty());
            assert!(!entry.value.is_empty());
            assert!(entry.package.starts_with("pkgs."), "{}", entry.package);
        }
    }

    #[test]
    fn shipped_catalog_has_previews() {
        assert!(fonts().len() >= 40);
        assert!(icon_themes().len() >= 10);
        assert!(cursor_themes().len() >= 3);
        for entry in fonts() {
            assert!(entry.preview.is_some(), "{}", entry.value);
        }
        for entry in icon_themes().iter().chain(cursor_themes()) {
            assert!(!entry.previews.is_empty(), "{}", entry.value);
        }
    }
}
