use super::context::{BuildContext, ChangeHook, FileActions};
use super::editor::Editor;
use super::nix::{
    nix_path_literal, normalize_path, parse_string_literal, relative_path, unwrap_modifiers,
};
use crate::dsl::OptionSpec;
use crate::model::Record;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Once;

pub(crate) const DEFAULT_FILE_DIR: &str = "./files";

pub(crate) const DEFAULT_FILE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "svg", "txt", "md", "markdown", "json", "yaml",
    "yml", "toml", "ini", "conf", "cfg", "nix", "sh", "css", "csv", "xml", "log",
];

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "svg"];
const TILE_WIDTH: i32 = 224;
const TILE_HEIGHT: i32 = 140;
const TEXT_PREVIEW_BYTES: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileEntry {
    name: String,
    value: String,
    absolute: PathBuf,
}

pub(crate) struct FileUi {
    current: Option<FileEntry>,
    tiles: Vec<FileTile>,
}

impl FileUi {
    pub(crate) fn nix_value(&self) -> String {
        match &self.current {
            Some(file) => file.value.clone(),
            None => "null".to_string(),
        }
    }
}

struct FileTile {
    button: gtk::Button,
    entry: Option<FileEntry>,
}

#[derive(Clone)]
struct FileView {
    strip: gtk::Box,
    scrolled: gtk::ScrolledWindow,
    ui: Rc<RefCell<FileUi>>,
    module_dir: PathBuf,
    dir: PathBuf,
    extensions: Vec<String>,
    remove: gtk::Button,
    on_change: ChangeHook,
    actions: FileActions,
    nullable: bool,
}

pub(crate) fn build_file(record: &Record, value_text: &str, ctx: &BuildContext) -> Editor {
    let Some(actions) = ctx.file.clone() else {
        return Editor::Path(super::basic::build_entry(value_text, &record.spec, false));
    };

    install_css();

    let module_dir = module_dir(record, ctx);
    let dir = file_dir(&record.spec, &module_dir);
    let extensions = file_extensions(&record.spec);
    let ui = Rc::new(RefCell::new(FileUi {
        current: initial_entry(value_text, &module_dir),
        tiles: Vec::new(),
    }));

    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    strip.add_css_class("file-strip");
    strip.set_halign(gtk::Align::Start);

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(TILE_HEIGHT + 60)
        .child(&strip)
        .build();
    scrolled.add_css_class("file-scroller");
    scrolled.set_hexpand(true);
    scrolled.set_focusable(true);

    let add = gtk::Button::with_label("Add files");
    let remove = gtk::Button::with_label("Remove file");
    add.add_css_class("pill");
    remove.add_css_class("pill");

    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let view = FileView {
        strip,
        scrolled,
        ui,
        module_dir,
        dir,
        extensions,
        remove: remove.clone(),
        on_change: on_change.clone(),
        actions,
        nullable: record.spec.nullable,
    };

    {
        let view = view.clone();
        let button = add.clone();
        add.connect_clicked(move |_| view.open_add(&button));
    }
    {
        let view = view.clone();
        let button = remove.clone();
        remove.connect_clicked(move |_| view.open_remove(&button));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    root.append(&view.scrolled);
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    toolbar.append(&add);
    toolbar.append(&remove);
    root.append(&toolbar);

    {
        let view = view.clone();
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gtk::gdk::Key::Left => {
                view.move_selection(-1);
                gtk::glib::Propagation::Stop
            }
            gtk::gdk::Key::Right => {
                view.move_selection(1);
                gtk::glib::Propagation::Stop
            }
            _ => gtk::glib::Propagation::Proceed,
        });
        root.add_controller(keys);
    }

    view.refresh();
    Editor::File {
        root,
        ui: view.ui.clone(),
        on_change,
    }
}

impl FileView {
    fn refresh(&self) {
        while let Some(child) = self.strip.first_child() {
            self.strip.remove(&child);
        }

        let mut tiles = Vec::new();
        if self.nullable {
            let button = self.build_tile(None);
            self.strip.append(&button);
            tiles.push(FileTile {
                button,
                entry: None,
            });
        }

        for entry in list_files(&self.dir, &self.module_dir, &self.extensions) {
            let button = self.build_tile(Some(entry.clone()));
            self.strip.append(&button);
            tiles.push(FileTile {
                button,
                entry: Some(entry),
            });
        }

        self.ui.borrow_mut().tiles = tiles;
        self.apply_selection();
    }

    fn build_tile(&self, entry: Option<FileEntry>) -> gtk::Button {
        let name = entry
            .as_ref()
            .map(|file| file.name.as_str())
            .unwrap_or("None");
        let button = gtk::Button::new();
        button.add_css_class("file-tile");
        button.set_tooltip_text(Some(name));

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.set_size_request(TILE_WIDTH, TILE_HEIGHT + 48);
        body.append(&self.preview(&entry));

        let label = gtk::Label::builder()
            .label(name)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(24)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(4)
            .margin_end(4)
            .build();
        body.append(&label);
        button.set_child(Some(&body));

        let view = self.clone();
        button.connect_clicked(move |_| {
            view.select(entry.clone());
            view.scrolled.grab_focus();
        });
        button
    }

    fn preview(&self, entry: &Option<FileEntry>) -> gtk::Box {
        let host = gtk::Box::new(gtk::Orientation::Vertical, 0);
        host.set_size_request(TILE_WIDTH, TILE_HEIGHT);
        host.set_overflow(gtk::Overflow::Hidden);
        host.add_css_class("file-preview");
        host.set_halign(gtk::Align::Center);
        host.set_valign(gtk::Align::Center);

        match entry {
            None => host.append(&placeholder("edit-clear-symbolic")),
            Some(file) if is_image(&file.absolute) => {
                if !append_image_preview(&host, &file.absolute) {
                    host.append(&placeholder("image-x-generic-symbolic"));
                }
            }
            Some(file) => host.append(&text_preview(&file.absolute)),
        }
        host
    }

    fn select(&self, entry: Option<FileEntry>) {
        self.ui.borrow_mut().current = entry;
        self.apply_selection();
        self.fire();
    }

    fn move_selection(&self, step: i32) {
        let (len, index) = {
            let ui = self.ui.borrow();
            let index = ui
                .tiles
                .iter()
                .position(|tile| same_entry(&tile.entry, &ui.current));
            (ui.tiles.len(), index)
        };
        if len == 0 {
            return;
        }
        let next = match index {
            Some(index) => (index as i32 + step).clamp(0, len as i32 - 1) as usize,
            None if step < 0 => len - 1,
            None => 0,
        };
        let entry = self.ui.borrow().tiles[next].entry.clone();
        self.select(entry);
    }

    fn apply_selection(&self) {
        let current = self.ui.borrow().current.clone();
        let can_remove = current
            .as_ref()
            .map(|file| file.absolute.starts_with(&self.dir))
            .unwrap_or(false);
        let selected = {
            let ui = self.ui.borrow();
            let mut selected = None;
            for tile in &ui.tiles {
                if same_entry(&tile.entry, &current) {
                    tile.button.add_css_class("file-selected");
                    selected = Some(tile.button.clone());
                } else {
                    tile.button.remove_css_class("file-selected");
                }
            }
            selected
        };
        self.remove.set_sensitive(can_remove);
        if let Some(button) = selected {
            self.scroll_to(&button);
        }
    }

    fn scroll_to(&self, button: &gtk::Button) {
        let scrolled = self.scrolled.clone();
        let strip = self.strip.clone();
        let button = button.clone();
        gtk::glib::idle_add_local_once(move || {
            let Some(bounds) = button.compute_bounds(&strip) else {
                return;
            };
            let adjustment = scrolled.hadjustment();
            let left = adjustment.value();
            let right = left + adjustment.page_size();
            let x = bounds.x() as f64;
            let width = bounds.width() as f64;
            if x < left {
                adjustment.set_value(x);
            } else if x + width > right {
                adjustment.set_value(x + width - adjustment.page_size());
            }
        });
    }

    fn fire(&self) {
        let hook = self.on_change.borrow().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    fn fallback_entry(&self) -> Option<FileEntry> {
        if self.nullable {
            return None;
        }
        list_files(&self.dir, &self.module_dir, &self.extensions)
            .into_iter()
            .next()
    }

    fn open_add(&self, button: &gtk::Button) {
        let dialog = gtk::FileDialog::builder()
            .title("Add files")
            .modal(true)
            .build();

        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        if !self.extensions.is_empty() {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Allowed files"));
            for extension in &self.extensions {
                filter.add_pattern(&format!("*.{}", extension.trim_start_matches('.')));
            }
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            dialog.set_default_filter(Some(&filter));
        }

        let parent = button.root().and_downcast::<gtk::Window>();
        let view = self.clone();
        let error_parent = parent.clone();
        dialog.open_multiple(
            parent.as_ref(),
            None::<&gtk::gio::Cancellable>,
            move |result| {
                let Ok(files) = result else {
                    return;
                };
                let mut added = None;
                let mut errors = Vec::new();
                for index in 0..files.n_items() {
                    let Some(file) = files.item(index).and_downcast::<gtk::gio::File>() else {
                        continue;
                    };
                    let Some(path) = file.path() else {
                        continue;
                    };
                    match (view.actions.add)(&path, &view.dir) {
                        Ok(path) => added = Some(file_entry(&view.module_dir, &path)),
                        Err(error) => errors.push(error),
                    }
                }
                if let Some(entry) = added {
                    view.ui.borrow_mut().current = Some(entry);
                    view.refresh();
                    view.fire();
                }
                if !errors.is_empty() {
                    show_error(error_parent.as_ref(), &errors.join("\n"));
                }
            },
        );
    }

    fn open_remove(&self, button: &gtk::Button) {
        let Some(file) = self.ui.borrow().current.clone() else {
            return;
        };
        if !file.absolute.starts_with(&self.dir) {
            return;
        }

        let dialog = gtk::MessageDialog::builder()
            .modal(true)
            .message_type(gtk::MessageType::Question)
            .buttons(gtk::ButtonsType::Cancel)
            .text(format!("Remove {}?", file.name))
            .secondary_text("The file is deleted from the repository.")
            .build();
        if let Some(parent) = button.root().and_downcast::<gtk::Window>() {
            dialog.set_transient_for(Some(&parent));
        }
        dialog.add_button("Remove", gtk::ResponseType::Accept);

        let view = self.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                match (view.actions.remove)(&file.absolute) {
                    Ok(()) => {
                        view.ui.borrow_mut().current = view.fallback_entry();
                        view.refresh();
                        view.fire();
                    }
                    Err(error) => show_error(dialog.transient_for().as_ref(), &error),
                }
            }
            dialog.close();
        });
        dialog.present();
    }
}

pub(crate) fn file_dir(spec: &OptionSpec, module_dir: &Path) -> PathBuf {
    let configured = spec.file_dir.as_deref().unwrap_or(DEFAULT_FILE_DIR);
    let path = Path::new(configured);
    if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&module_dir.join(path))
    }
}

pub(crate) fn file_extensions(spec: &OptionSpec) -> Vec<String> {
    spec.file_extensions
        .clone()
        .unwrap_or_else(|| {
            DEFAULT_FILE_EXTENSIONS
                .iter()
                .map(|extension| extension.to_string())
                .collect()
        })
        .into_iter()
        .map(|extension| extension.trim().trim_start_matches('.').to_lowercase())
        .filter(|extension| !extension.is_empty())
        .collect()
}

fn module_dir(record: &Record, ctx: &BuildContext) -> PathBuf {
    Path::new(&record.file)
        .parent()
        .map(normalize_path)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| normalize_path(Path::new(&ctx.base)))
}

fn parse_path_expression(text: &str) -> Option<String> {
    let inner = text.trim().strip_prefix('(')?.strip_suffix(')')?;
    let (parent, name) = inner.split_once('+')?;
    let parent = parent.trim();
    if !(parent.starts_with("./") || parent.starts_with("../") || parent.starts_with('/')) {
        return None;
    }
    let name = name.trim();
    if !(name.starts_with('"') && name.ends_with('"')) {
        return None;
    }
    Some(format!("{parent}{}", parse_string_literal(name)))
}

fn initial_entry(value_text: &str, module_dir: &Path) -> Option<FileEntry> {
    let text = unwrap_modifiers(value_text);
    let text = text.trim();
    if text.is_empty() || text == "null" {
        return None;
    }
    let expression = parse_path_expression(text);
    let raw = expression
        .clone()
        .unwrap_or_else(|| parse_string_literal(text));
    let path = Path::new(&raw);
    let absolute = if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&module_dir.join(path))
    };
    let value = if expression.is_some() {
        text.to_string()
    } else if (text.starts_with('"') || text.starts_with("''")) && !path.is_absolute() {
        nix_path_literal(&raw)
    } else if text.starts_with('"') || text.starts_with("''") {
        text.to_string()
    } else {
        nix_path_literal(text)
    };
    Some(FileEntry {
        name: file_name(&absolute),
        value,
        absolute,
    })
}

fn list_files(dir: &Path, module_dir: &Path, extensions: &[String]) -> Vec<FileEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<FileEntry> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && has_extension(path, extensions))
        .map(|path| file_entry(module_dir, &path))
        .collect();
    files.sort_by_key(|file| file.name.to_lowercase());
    files
}

fn has_extension(path: &Path, extensions: &[String]) -> bool {
    if extensions.is_empty() {
        return true;
    }
    path.extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .map(|extension| {
            extensions
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(&extension))
        })
        .unwrap_or(false)
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .map(|extension| IMAGE_EXTENSIONS.contains(&extension.as_str()))
        .unwrap_or(false)
}

fn file_entry(module_dir: &Path, path: &Path) -> FileEntry {
    let path = normalize_path(path);
    FileEntry {
        name: file_name(&path),
        value: nix_path_literal(&relative_path(module_dir, &path)),
        absolute: path,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn same_entry(a: &Option<FileEntry>, b: &Option<FileEntry>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.value == b.value || a.absolute == b.absolute,
        (None, None) => true,
        _ => false,
    }
}

fn append_image_preview(host: &gtk::Box, path: &Path) -> bool {
    match gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, TILE_WIDTH, TILE_HEIGHT, true) {
        Ok(pixbuf) => {
            let picture = gtk::Picture::for_pixbuf(&pixbuf);
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_can_shrink(true);
            picture.set_size_request(TILE_WIDTH, TILE_HEIGHT);
            host.append(&picture);
            true
        }
        Err(_) => false,
    }
}

fn text_preview(path: &Path) -> gtk::Label {
    let text = std::fs::read(path)
        .map(|bytes| text_excerpt(&bytes))
        .unwrap_or_default();
    let shown = if text.is_empty() {
        "(empty file)".to_string()
    } else {
        text
    };
    let label = gtk::Label::builder()
        .label(&shown)
        .xalign(0.0)
        .yalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .lines(6)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();
    label.add_css_class("file-text-preview");
    label
}

fn text_excerpt(bytes: &[u8]) -> String {
    let end = bytes.len().min(TEXT_PREVIEW_BYTES);
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

fn placeholder(icon_name: &str) -> gtk::Image {
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(48);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_vexpand(true);
    icon
}

fn show_error(parent: Option<&gtk::Window>, message: &str) {
    crate::error_dialog::show(parent, message);
}

fn install_css() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let css = ".file-strip { padding: 2px; }
.file-tile {
  padding: 0;
  border: 2px solid transparent;
  border-radius: 8px;
  background: alpha(currentColor, 0.06);
}
.file-tile.file-selected {
  border-color: @accent_bg_color;
  background: alpha(@accent_bg_color, 0.14);
}
.file-preview { border-radius: 6px; background: alpha(currentColor, 0.10); }
.file-text-preview { font-family: monospace; font-size: 0.85em; }
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

    fn spec(dir: Option<&str>, extensions: Option<&[&str]>) -> OptionSpec {
        OptionSpec {
            file_dir: dir.map(str::to_string),
            file_extensions: extensions.map(|list| list.iter().map(|s| s.to_string()).collect()),
            ..OptionSpec::default()
        }
    }

    #[test]
    fn default_dir_is_next_to_the_page_file() {
        let directory = file_dir(&spec(None, None), Path::new("/repo/modules/02-core"));
        assert_eq!(directory, PathBuf::from("/repo/modules/02-core/files"));
    }

    #[test]
    fn explicit_relative_dir_is_next_to_the_page_file() {
        let directory = file_dir(
            &spec(Some("../assets/files"), None),
            Path::new("/repo/modules/02-core"),
        );
        assert_eq!(directory, PathBuf::from("/repo/modules/assets/files"));
    }

    #[test]
    fn explicit_absolute_dir_stays_absolute() {
        let directory = file_dir(&spec(Some("/srv/files"), None), Path::new("/repo"));
        assert_eq!(directory, PathBuf::from("/srv/files"));
    }

    #[test]
    fn default_extensions_cover_images_and_text() {
        let extensions = file_extensions(&spec(None, None));
        assert!(extensions.iter().any(|extension| extension == "png"));
        assert!(extensions.iter().any(|extension| extension == "txt"));
    }

    #[test]
    fn explicit_extensions_are_normalized() {
        let extensions = file_extensions(&spec(None, Some(&[".PNG", " md ", ""])));
        assert_eq!(extensions, vec!["png".to_string(), "md".to_string()]);
    }

    #[test]
    fn extension_matching_ignores_case() {
        let extensions = vec!["png".to_string(), "md".to_string()];
        assert!(has_extension(Path::new("/repo/files/a.PNG"), &extensions));
        assert!(has_extension(
            Path::new("/repo/files/notes.md"),
            &extensions
        ));
        assert!(!has_extension(Path::new("/repo/files/a.pdf"), &extensions));
    }

    #[test]
    fn null_value_starts_empty() {
        let entry = initial_entry("null", Path::new("/repo/modules/02-core"));
        assert!(entry.is_none());
        let ui = FileUi {
            current: None,
            tiles: Vec::new(),
        };
        assert_eq!(ui.nix_value(), "null");
    }

    #[test]
    fn relative_value_resolves_to_an_absolute_path() {
        let entry = initial_entry("./files/notes.txt", Path::new("/repo/modules/02-core")).unwrap();
        assert_eq!(
            entry.absolute,
            PathBuf::from("/repo/modules/02-core/files/notes.txt")
        );
        assert_eq!(entry.value, "./files/notes.txt");
    }

    #[test]
    fn file_entries_serialize_as_relative_paths() {
        let entry = file_entry(
            Path::new("/repo/modules/02-core"),
            Path::new("/repo/modules/02-core/files/notes.txt"),
        );
        assert_eq!(entry.name, "notes.txt");
        assert_eq!(entry.value, "./files/notes.txt");
    }

    #[test]
    fn whitespace_paths_become_path_expressions() {
        let entry = file_entry(
            Path::new("/repo/modules/02-core"),
            Path::new("/repo/modules/02-core/files/my notes.txt"),
        );
        assert_eq!(entry.value, "(./files + \"/my notes.txt\")");
        let round_trip = initial_entry(
            "(./files + \"/my notes.txt\")",
            Path::new("/repo/modules/02-core"),
        )
        .unwrap();
        assert_eq!(
            round_trip.absolute,
            PathBuf::from("/repo/modules/02-core/files/my notes.txt")
        );
        assert_eq!(round_trip.value, "(./files + \"/my notes.txt\")");
    }

    #[test]
    fn text_excerpt_truncates_long_files() {
        let bytes = vec![b'a'; TEXT_PREVIEW_BYTES + 10];
        assert_eq!(text_excerpt(&bytes).len(), TEXT_PREVIEW_BYTES);
        assert_eq!(text_excerpt(b"  hello\n"), "hello");
    }

    #[test]
    fn ui_serializes_the_selected_entry() {
        let entry = file_entry(
            Path::new("/repo/modules/02-core"),
            Path::new("/repo/modules/02-core/files/notes.txt"),
        );
        let ui = FileUi {
            current: Some(entry),
            tiles: Vec::new(),
        };
        assert_eq!(ui.nix_value(), "./files/notes.txt");
    }
}
