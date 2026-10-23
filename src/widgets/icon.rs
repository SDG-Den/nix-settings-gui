use super::context::{BuildContext, ChangeHook};
use super::editor::Editor;
use super::nix::{nix_path_literal, normalize_path, parse_string_literal, relative_path};
use crate::dsl::OptionSpec;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Once;

pub(crate) const DEFAULT_ICON_DIR: &str = "./icons";
const PREVIEW_SIZE: i32 = 48;

#[derive(Clone, Debug, PartialEq, Eq)]
enum IconChoice {
    Empty,
    File { value: String, absolute: PathBuf },
}

impl IconChoice {
    fn nix_value(&self) -> String {
        match self {
            IconChoice::Empty => "null".to_string(),
            IconChoice::File { value, .. } => value.clone(),
        }
    }
}

pub(crate) struct IconUi {
    current: IconChoice,
}

impl IconUi {
    pub(crate) fn nix_value(&self) -> String {
        self.current.nix_value()
    }
}

#[derive(Clone)]
struct IconView {
    preview: gtk::Box,
    ui: Rc<RefCell<IconUi>>,
    module_dir: PathBuf,
    dir: PathBuf,
    on_change: ChangeHook,
    actions: super::context::IconActions,
}

pub(crate) fn icon_dir(spec: &OptionSpec, module_dir: &Path) -> PathBuf {
    let configured = spec.icon_dir.as_deref().unwrap_or(DEFAULT_ICON_DIR);
    let path = Path::new(configured);
    if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&module_dir.join(path))
    }
}

pub(crate) fn build_icon(
    value_text: &str,
    module_dir: &Path,
    dir: &Path,
    ctx: &BuildContext,
) -> Editor {
    let Some(actions) = ctx.icon.clone() else {
        return Editor::Path(super::basic::build_entry(
            value_text,
            &OptionSpec::default(),
            false,
        ));
    };

    install_css();

    let ui = Rc::new(RefCell::new(IconUi {
        current: initial_choice(value_text, module_dir),
    }));
    let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview.set_size_request(PREVIEW_SIZE, PREVIEW_SIZE);
    preview.set_overflow(gtk::Overflow::Hidden);
    preview.add_css_class("icon-preview");
    preview.set_valign(gtk::Align::Center);

    let add = gtk::Button::with_label("Add icon");
    add.add_css_class("pill");
    add.set_valign(gtk::Align::Center);

    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let view = IconView {
        preview,
        ui,
        module_dir: module_dir.to_path_buf(),
        dir: dir.to_path_buf(),
        on_change: on_change.clone(),
        actions,
    };
    {
        let view = view.clone();
        let button = add.clone();
        add.connect_clicked(move |_| view.open_add(&button));
    }

    view.refresh();

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    root.set_hexpand(true);
    root.append(&view.preview);
    root.append(&add);

    Editor::Icon {
        root,
        ui: view.ui.clone(),
        on_change,
    }
}

impl IconView {
    fn refresh(&self) {
        while let Some(child) = self.preview.first_child() {
            self.preview.remove(&child);
        }
        let current = &self.ui.borrow().current;
        match current {
            IconChoice::Empty => {
                self.preview.set_tooltip_text(Some("No icon"));
                self.preview.append(&placeholder());
            }
            IconChoice::File { absolute, .. } => {
                self.preview.set_tooltip_text(
                    absolute
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .as_deref(),
                );
                match gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(
                    absolute,
                    PREVIEW_SIZE,
                    PREVIEW_SIZE,
                    true,
                ) {
                    Ok(pixbuf) => {
                        let picture = gtk::Picture::for_pixbuf(&pixbuf);
                        picture.set_content_fit(gtk::ContentFit::Cover);
                        picture.set_can_shrink(true);
                        picture.set_size_request(PREVIEW_SIZE, PREVIEW_SIZE);
                        self.preview.append(&picture);
                    }
                    Err(_) => self.preview.append(&placeholder()),
                }
            }
        }
    }

    fn fire(&self) {
        let hook = self.on_change.borrow().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    fn open_add(&self, button: &gtk::Button) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        filter.add_mime_type("image/*");
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title("Add icon")
            .modal(true)
            .build();
        dialog.set_filters(Some(&filters));
        dialog.set_default_filter(Some(&filter));

        let parent = button.root().and_downcast::<gtk::Window>();
        let view = self.clone();
        let error_parent = parent.clone();
        dialog.open(
            parent.as_ref(),
            None::<&gtk::gio::Cancellable>,
            move |result| {
                let Ok(file) = result else {
                    return;
                };
                let Some(path) = file.path() else {
                    return;
                };
                match (view.actions.add)(&path, &view.dir) {
                    Ok(path) => {
                        let absolute = normalize_path(&path);
                        view.ui.borrow_mut().current = IconChoice::File {
                            value: nix_path_literal(&relative_path(&view.module_dir, &absolute)),
                            absolute,
                        };
                        view.refresh();
                        view.fire();
                    }
                    Err(error) => show_error(error_parent.as_ref(), &error),
                }
            },
        );
    }
}

fn initial_choice(value_text: &str, module_dir: &Path) -> IconChoice {
    let text = value_text.trim();
    if text.is_empty() || text == "null" {
        return IconChoice::Empty;
    }
    let raw = parse_string_literal(text);
    let path = Path::new(&raw);
    let absolute = if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&module_dir.join(path))
    };
    let value = if text.starts_with('"') || text.starts_with("''") {
        text.to_string()
    } else {
        nix_path_literal(text)
    };
    IconChoice::File { value, absolute }
}

fn placeholder() -> gtk::Image {
    let icon = gtk::Image::from_icon_name("image-x-generic-symbolic");
    icon.set_pixel_size(PREVIEW_SIZE / 2);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_vexpand(true);
    icon
}

fn show_error(parent: Option<&gtk::Window>, message: &str) {
    let dialog = gtk::MessageDialog::builder()
        .modal(true)
        .message_type(gtk::MessageType::Error)
        .buttons(gtk::ButtonsType::Close)
        .text(message)
        .build();
    if let Some(parent) = parent {
        dialog.set_transient_for(Some(parent));
    }
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

fn install_css() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let css = ".icon-preview { border-radius: 6px; background: alpha(currentColor, 0.10); }
";
        let provider = gtk::CssProvider::new();
        provider.load_from_data(css);
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

    fn spec(dir: Option<&str>) -> OptionSpec {
        OptionSpec {
            icon_dir: dir.map(str::to_string),
            ..OptionSpec::default()
        }
    }

    #[test]
    fn default_dir_is_next_to_the_module_file() {
        let directory = icon_dir(&spec(None), Path::new("/repo/modules/06-utility"));
        assert_eq!(directory, PathBuf::from("/repo/modules/06-utility/icons"));
    }

    #[test]
    fn explicit_relative_dir_is_next_to_the_module_file() {
        let directory = icon_dir(
            &spec(Some("../assets/icons")),
            Path::new("/repo/modules/06-utility"),
        );
        assert_eq!(directory, PathBuf::from("/repo/modules/assets/icons"));
    }

    #[test]
    fn explicit_absolute_dir_stays_absolute() {
        let directory = icon_dir(&spec(Some("/srv/icons")), Path::new("/repo"));
        assert_eq!(directory, PathBuf::from("/srv/icons"));
    }

    #[test]
    fn null_value_starts_empty() {
        let choice = initial_choice("null", Path::new("/repo/modules/06-utility"));
        assert_eq!(choice, IconChoice::Empty);
        assert_eq!(choice.nix_value(), "null");
    }

    #[test]
    fn relative_value_resolves_against_the_module_file() {
        let choice = initial_choice("./icons/youtube.png", Path::new("/repo/modules/06-utility"));
        let IconChoice::File { value, absolute } = &choice else {
            panic!("expected a file");
        };
        assert_eq!(
            absolute,
            &PathBuf::from("/repo/modules/06-utility/icons/youtube.png")
        );
        assert_eq!(value, "./icons/youtube.png");
        assert_eq!(choice.nix_value(), "./icons/youtube.png");
    }

    #[test]
    fn quoted_values_keep_their_quotes() {
        let choice = initial_choice(
            "\"./icons/my icon.png\"",
            Path::new("/repo/modules/06-utility"),
        );
        let IconChoice::File { value, .. } = &choice else {
            panic!("expected a file");
        };
        assert_eq!(value, "\"./icons/my icon.png\"");
    }
}
