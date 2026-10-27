use super::context::{BuildContext, ChangeHook, WallpaperActions};
use super::editor::Editor;
use super::nix::{
    nix_path_literal, normalize_path, parse_string_literal, quote, relative_path,
    unwrap_modifiers,
};
use crate::dsl::OptionSpec;
use crate::model::Record;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Once;

pub(crate) const DEFAULT_WALLPAPER_DIR: &str = "./wallpapers";

const TILE_WIDTH: i32 = 176;
const PREVIEW_HEIGHT: i32 = 99;
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

#[derive(Clone, Debug, PartialEq, Eq)]
struct WallpaperFile {
    name: String,
    value: String,
    absolute: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WallpaperPresetChoice {
    name: String,
    label: String,
    path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WallpaperChoice {
    Auto,
    Preset(WallpaperPresetChoice),
    File(WallpaperFile),
}

impl WallpaperChoice {
    fn nix_value(&self) -> String {
        match self {
            WallpaperChoice::Auto => "null".to_string(),
            WallpaperChoice::Preset(preset) => quote(&preset.name),
            WallpaperChoice::File(file) => file.value.clone(),
        }
    }

    fn apply_literal(&self) -> String {
        match self {
            WallpaperChoice::Auto => "null".to_string(),
            WallpaperChoice::Preset(preset) => {
                if preset.path.as_os_str().is_empty() {
                    quote(&preset.name)
                } else {
                    quote(&preset.path.to_string_lossy())
                }
            }
            WallpaperChoice::File(file) => quote(&file.absolute.to_string_lossy()),
        }
    }
}

pub(crate) struct WallpaperUi {
    current: WallpaperChoice,
    tiles: Vec<WallpaperTile>,
    default_path: Option<String>,
}

impl WallpaperUi {
    pub(crate) fn nix_value(&self) -> String {
        self.current.nix_value()
    }

    pub(crate) fn apply_literal(&self) -> String {
        match &self.current {
            WallpaperChoice::Auto => self
                .default_path
                .as_ref()
                .map(|path| quote(path))
                .unwrap_or_else(|| "null".to_string()),
            _ => self.current.apply_literal(),
        }
    }

    fn can_remove(&self, dir: &Path) -> bool {
        match &self.current {
            WallpaperChoice::Auto | WallpaperChoice::Preset(_) => false,
            WallpaperChoice::File(file) => file.absolute.starts_with(dir),
        }
    }
}

struct WallpaperTile {
    button: gtk::Button,
    choice: WallpaperChoice,
}

#[derive(Clone)]
struct WallpaperView {
    strip: gtk::Box,
    scrolled: gtk::ScrolledWindow,
    ui: Rc<RefCell<WallpaperUi>>,
    presets: Rc<RefCell<Vec<WallpaperPresetChoice>>>,
    module_dir: PathBuf,
    dir: PathBuf,
    remove: gtk::Button,
    on_change: ChangeHook,
    actions: WallpaperActions,
    nullable: bool,
}

pub(crate) fn build_wallpaper(record: &Record, value_text: &str, ctx: &BuildContext) -> Editor {
    let Some(actions) = ctx.wallpaper.clone() else {
        return Editor::Path(super::basic::build_entry(value_text, &record.spec, false));
    };

    install_css();

    let module_dir = module_dir(record, ctx);
    let dir = wallpaper_dir(&record.spec, &module_dir);
    let current = initial_choice(value_text, &module_dir);
    let ui = Rc::new(RefCell::new(WallpaperUi {
        current,
        tiles: Vec::new(),
        default_path: None,
    }));
    let presets = Rc::new(RefCell::new(Vec::new()));
    if let WallpaperChoice::Preset(preset) = &ui.borrow().current {
        presets.borrow_mut().push(preset.clone());
    }

    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    strip.add_css_class("wallpaper-strip");
    strip.set_halign(gtk::Align::Start);

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(PREVIEW_HEIGHT + 56)
        .child(&strip)
        .build();
    scrolled.add_css_class("wallpaper-scroller");
    scrolled.set_hexpand(true);
    scrolled.set_focusable(true);

    let add = gtk::Button::with_label("Add wallpaper");
    let remove = gtk::Button::with_label("Remove wallpaper");
    add.add_css_class("pill");
    remove.add_css_class("pill");

    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let view = WallpaperView {
        strip,
        scrolled,
        ui,
        presets,
        module_dir,
        dir,
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
    {
        let view = view.clone();
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let scrolled = view.scrolled.clone();
        scroll.connect_scroll(move |controller, dx, dy| {
            if !scrolled.has_focus() {
                return gtk::glib::Propagation::Proceed;
            }
            let delta = if dx.abs() > f64::EPSILON { dx } else { dy };
            let adjustment = scrolled.hadjustment();
            let next =
                adjustment.value() + scroll_delta(delta, controller.unit(), adjustment.page_size());
            let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            adjustment.set_value(next.clamp(adjustment.lower(), upper));
            gtk::glib::Propagation::Stop
        });
        view.scrolled.add_controller(scroll);
    }

    if record.spec.preset_source.is_some() {
        if let Some(fetch) = ctx.wallpaper_set.clone() {
            let view = view.clone();
            fetch(Box::new(move |result| {
                if let Ok(set) = result {
                    view.set_presets(set);
                }
            }));
        }
    }

    view.refresh();
    Editor::Wallpaper {
        root,
        ui: view.ui.clone(),
        on_change,
    }
}

fn scroll_delta(delta: f64, unit: gtk::gdk::ScrollUnit, page: f64) -> f64 {
    match unit {
        gtk::gdk::ScrollUnit::Wheel => delta * page.powf(2.0 / 3.0),
        _ => delta * 2.5,
    }
}

impl WallpaperView {
    fn set_presets(&self, set: crate::metadata::WallpaperSet) {
        let current = self.ui.borrow().current.clone();
        *self.presets.borrow_mut() = set
            .presets
            .into_iter()
            .map(|preset| WallpaperPresetChoice {
                name: preset.name,
                label: preset.label,
                path: PathBuf::from(preset.path),
            })
            .collect();
        self.ui.borrow_mut().default_path =
            Some(set.default_path).filter(|path| !path.is_empty());
        if let WallpaperChoice::Preset(preset) = current {
            let known = self
                .presets
                .borrow()
                .iter()
                .find(|entry| entry.name == preset.name)
                .cloned();
            if let Some(known) = known {
                self.ui.borrow_mut().current = WallpaperChoice::Preset(known);
            } else {
                let file = self.dir.join(&preset.name);
                if file.is_file() {
                    self.ui.borrow_mut().current =
                        WallpaperChoice::File(wallpaper_file(&self.module_dir, &file));
                } else {
                    self.presets.borrow_mut().push(preset);
                }
            }
        }
        self.refresh();
    }

    fn refresh(&self) {
        while let Some(child) = self.strip.first_child() {
            self.strip.remove(&child);
        }

        let mut tiles = Vec::new();
        if self.nullable {
            let auto = self.build_tile(WallpaperChoice::Auto);
            self.strip.append(&auto);
            tiles.push(WallpaperTile {
                button: auto,
                choice: WallpaperChoice::Auto,
            });
        }

        for preset in self.presets.borrow().clone() {
            let choice = WallpaperChoice::Preset(preset);
            let button = self.build_tile(choice.clone());
            self.strip.append(&button);
            tiles.push(WallpaperTile { button, choice });
        }

        for file in list_images(&self.dir, &self.module_dir) {
            let choice = WallpaperChoice::File(file);
            let button = self.build_tile(choice.clone());
            self.strip.append(&button);
            tiles.push(WallpaperTile { button, choice });
        }

        self.ui.borrow_mut().tiles = tiles;
        self.apply_selection();
    }

    fn build_tile(&self, choice: WallpaperChoice) -> gtk::Button {
        let name = match &choice {
            WallpaperChoice::Auto => "Auto-themed",
            WallpaperChoice::Preset(preset) => &preset.label,
            WallpaperChoice::File(file) => &file.name,
        };
        let button = gtk::Button::new();
        button.add_css_class("wallpaper-tile");
        button.set_tooltip_text(Some(name));

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.set_size_request(TILE_WIDTH, PREVIEW_HEIGHT + 48);
        body.append(&self.preview(&choice));

        let label = gtk::Label::builder()
            .label(name)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(18)
            .margin_top(4)
            .margin_bottom(0)
            .margin_start(4)
            .margin_end(4)
            .build();
        body.append(&label);

        let kind = match &choice {
            WallpaperChoice::Auto => None,
            WallpaperChoice::Preset(_) => Some("Bundled"),
            WallpaperChoice::File(_) => Some("Custom"),
        };
        if let Some(kind) = kind {
            let badge = gtk::Label::builder()
                .label(kind)
                .margin_bottom(4)
                .margin_start(4)
                .margin_end(4)
                .build();
            badge.add_css_class("dim-label");
            badge.add_css_class("caption");
            body.append(&badge);
        }
        button.set_child(Some(&body));

        let view = self.clone();
        button.connect_clicked(move |_| {
            view.select(choice.clone());
            view.scrolled.grab_focus();
        });
        button
    }

    fn preview(&self, choice: &WallpaperChoice) -> gtk::Box {
        let host = gtk::Box::new(gtk::Orientation::Vertical, 0);
        host.set_size_request(TILE_WIDTH, PREVIEW_HEIGHT);
        host.set_overflow(gtk::Overflow::Hidden);
        host.add_css_class("wallpaper-preview");
        host.set_halign(gtk::Align::Center);
        host.set_valign(gtk::Align::Center);

        let fallback = || {
            let icon = gtk::Image::from_icon_name("image-x-generic-symbolic");
            icon.set_pixel_size(40);
            icon.set_halign(gtk::Align::Center);
            icon.set_valign(gtk::Align::Center);
            icon.set_vexpand(true);
            icon
        };

        match choice {
            WallpaperChoice::Auto => {
                let default_path = self.ui.borrow().default_path.clone();
                if let Some(path) = default_path.filter(|path| !path.is_empty()) {
                    append_preview(&host, Path::new(&path), &fallback);
                } else {
                    let icon = gtk::Image::from_icon_name("applications-graphics-symbolic");
                    icon.set_pixel_size(40);
                    icon.set_halign(gtk::Align::Center);
                    icon.set_valign(gtk::Align::Center);
                    icon.set_vexpand(true);
                    host.append(&icon);
                }
            }
            WallpaperChoice::Preset(preset) if preset.path.as_os_str().is_empty() => {
                host.append(&fallback());
            }
            WallpaperChoice::Preset(preset) => append_preview(&host, &preset.path, &fallback),
            WallpaperChoice::File(file) => append_preview(&host, &file.absolute, &fallback),
        }
        host
    }

    fn select(&self, choice: WallpaperChoice) {
        self.ui.borrow_mut().current = choice;
        self.apply_selection();
        self.fire();
    }

    fn move_selection(&self, step: i32) {
        let (len, index) = {
            let ui = self.ui.borrow();
            let index = ui
                .tiles
                .iter()
                .position(|tile| same_choice(&tile.choice, &ui.current));
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
        let choice = self.ui.borrow().tiles[next].choice.clone();
        self.select(choice);
    }

    fn apply_selection(&self) {
        let current = self.ui.borrow().current.clone();
        let can_remove = self.ui.borrow().can_remove(&self.dir);
        let selected = {
            let ui = self.ui.borrow();
            let mut selected = None;
            for tile in &ui.tiles {
                if same_choice(&tile.choice, &current) {
                    tile.button.add_css_class("wallpaper-selected");
                    selected = Some(tile.button.clone());
                } else {
                    tile.button.remove_css_class("wallpaper-selected");
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

    fn open_add(&self, button: &gtk::Button) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        filter.add_mime_type("image/*");
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title("Add wallpaper")
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
                        view.ui.borrow_mut().current =
                            WallpaperChoice::File(wallpaper_file(&view.module_dir, &path));
                        view.refresh();
                        view.fire();
                    }
                    Err(error) => show_error(error_parent.as_ref(), &error),
                }
            },
        );
    }

    fn fallback_choice(&self) -> WallpaperChoice {
        if self.nullable {
            return WallpaperChoice::Auto;
        }
        if let Some(preset) = self.presets.borrow().first().cloned() {
            return WallpaperChoice::Preset(preset);
        }
        if let Some(file) = list_images(&self.dir, &self.module_dir).into_iter().next() {
            return WallpaperChoice::File(file);
        }
        WallpaperChoice::Auto
    }

    fn open_remove(&self, button: &gtk::Button) {
        let choice = self.ui.borrow().current.clone();
        let WallpaperChoice::File(file) = choice else {
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
                        view.ui.borrow_mut().current = view.fallback_choice();
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

fn append_preview(host: &gtk::Box, path: &Path, fallback: &impl Fn() -> gtk::Image) {
    match gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, TILE_WIDTH, PREVIEW_HEIGHT, true) {
        Ok(pixbuf) => {
            let picture = gtk::Picture::for_pixbuf(&pixbuf);
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_can_shrink(true);
            picture.set_size_request(TILE_WIDTH, PREVIEW_HEIGHT);
            host.append(&picture);
        }
        Err(_) => host.append(&fallback()),
    }
}

fn show_error(parent: Option<&gtk::Window>, message: &str) {
    crate::error_dialog::show(parent, message);
}

fn same_choice(a: &WallpaperChoice, b: &WallpaperChoice) -> bool {
    match (a, b) {
        (WallpaperChoice::Auto, WallpaperChoice::Auto) => true,
        (WallpaperChoice::Preset(a), WallpaperChoice::Preset(b)) => a.name == b.name,
        (WallpaperChoice::File(a), WallpaperChoice::File(b)) => {
            a.value == b.value || a.absolute == b.absolute
        }
        _ => false,
    }
}

fn looks_like_preset(value: &str) -> bool {
    !value.contains('/')
        && !value.contains('\\')
        && !value.starts_with('.')
        && Path::new(value)
            .extension()
            .map(|extension| {
                !IMAGE_EXTENSIONS.contains(&extension.to_string_lossy().to_lowercase().as_str())
            })
            .unwrap_or(true)
}

fn module_dir(record: &Record, ctx: &BuildContext) -> PathBuf {
    Path::new(&record.file)
        .parent()
        .map(normalize_path)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| normalize_path(Path::new(&ctx.base)))
}

pub(crate) fn wallpaper_dir(spec: &OptionSpec, module_dir: &Path) -> PathBuf {
    let configured = spec
        .wallpaper_dir
        .as_deref()
        .unwrap_or(DEFAULT_WALLPAPER_DIR);
    let path = Path::new(configured);
    if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&module_dir.join(path))
    }
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

fn initial_choice(value_text: &str, module_dir: &Path) -> WallpaperChoice {
    let text = unwrap_modifiers(value_text);
    let text = text.trim();
    if text.is_empty() || text == "null" {
        return WallpaperChoice::Auto;
    }
    let expression = parse_path_expression(text);
    let raw = expression
        .clone()
        .unwrap_or_else(|| parse_string_literal(text));
    if expression.is_none() && looks_like_preset(&raw) {
        return WallpaperChoice::Preset(WallpaperPresetChoice {
            name: raw.clone(),
            label: raw,
            path: PathBuf::new(),
        });
    }
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
    WallpaperChoice::File(WallpaperFile {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.clone()),
        value,
        absolute,
    })
}

fn list_images(dir: &Path, module_dir: &Path) -> Vec<WallpaperFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut images: Vec<WallpaperFile> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_image(path))
        .map(|path| wallpaper_file(module_dir, &path))
        .collect();
    images.sort_by_key(|image| image.name.to_lowercase());
    images
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .map(|extension| IMAGE_EXTENSIONS.contains(&extension.as_str()))
        .unwrap_or(false)
}

fn wallpaper_file(module_dir: &Path, path: &Path) -> WallpaperFile {
    let path = normalize_path(path);
    WallpaperFile {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        value: nix_path_literal(&relative_path(module_dir, &path)),
        absolute: path,
    }
}

fn install_css() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let css = ".wallpaper-strip { padding: 2px; }
.wallpaper-tile {
  padding: 0;
  border: 2px solid transparent;
  border-radius: 8px;
  background: alpha(currentColor, 0.06);
}
.wallpaper-tile.wallpaper-selected {
  border-color: @accent_bg_color;
  background: alpha(@accent_bg_color, 0.14);
}
.wallpaper-preview { border-radius: 6px; background: alpha(currentColor, 0.10); }
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
            wallpaper_dir: dir.map(str::to_string),
            ..OptionSpec::default()
        }
    }

    #[test]
    fn default_dir_is_next_to_the_module_file() {
        let directory = wallpaper_dir(&spec(None), Path::new("/repo/modules/02-core"));
        assert_eq!(directory, PathBuf::from("/repo/modules/02-core/wallpapers"));
    }

    #[test]
    fn explicit_relative_dir_is_next_to_the_module_file() {
        let directory = wallpaper_dir(
            &spec(Some("../assets/wallpapers")),
            Path::new("/repo/modules/02-core"),
        );
        assert_eq!(directory, PathBuf::from("/repo/modules/assets/wallpapers"));
    }

    #[test]
    fn explicit_absolute_dir_stays_absolute() {
        let directory = wallpaper_dir(&spec(Some("/srv/wallpapers")), Path::new("/repo"));
        assert_eq!(directory, PathBuf::from("/srv/wallpapers"));
    }

    #[test]
    fn relative_literals_are_module_relative() {
        let value = relative_path(
            Path::new("/repo/modules/02-core"),
            Path::new("/repo/modules/02-core/wallpapers/everforest.png"),
        );
        assert_eq!(value, "./wallpapers/everforest.png");
    }

    #[test]
    fn null_value_selects_auto_themed() {
        let choice = initial_choice("null", Path::new("/repo/modules/02-core"));
        assert_eq!(choice, WallpaperChoice::Auto);
        assert_eq!(choice.nix_value(), "null");
        assert_eq!(choice.apply_literal(), "null");
    }

    #[test]
    fn default_modifiers_are_unwrapped() {
        let auto = initial_choice("lib.mkDefault null", Path::new("/repo/modules/02-core"));
        assert_eq!(auto, WallpaperChoice::Auto);
        let preset = initial_choice(
            "lib.mkDefault \"stripes\"",
            Path::new("/repo/modules/02-core"),
        );
        assert_eq!(preset.nix_value(), "\"stripes\"");
    }

    #[test]
    fn preset_names_are_not_treated_as_paths() {
        let choice = initial_choice("stripes", Path::new("/repo/modules/02-core"));
        let WallpaperChoice::Preset(preset) = &choice else {
            panic!("expected a preset");
        };
        assert_eq!(preset.name, "stripes");
        assert_eq!(choice.nix_value(), "\"stripes\"");
        assert_eq!(choice.apply_literal(), "\"stripes\"");
    }

    #[test]
    fn relative_value_resolves_to_an_absolute_apply_path() {
        let choice = initial_choice(
            "./wallpapers/everforest.png",
            Path::new("/repo/modules/02-core"),
        );
        let WallpaperChoice::File(file) = &choice else {
            panic!("expected a file");
        };
        assert_eq!(
            file.absolute,
            PathBuf::from("/repo/modules/02-core/wallpapers/everforest.png")
        );
        assert_eq!(choice.nix_value(), "./wallpapers/everforest.png");
        assert_eq!(
            choice.apply_literal(),
            "\"/repo/modules/02-core/wallpapers/everforest.png\""
        );
    }

    #[test]
    fn file_values_from_the_directory_keep_the_relative_literal() {
        let file = wallpaper_file(
            Path::new("/repo/modules/02-core"),
            Path::new("/repo/modules/02-core/wallpapers/everforest.png"),
        );
        assert_eq!(file.name, "everforest.png");
        assert_eq!(file.value, "./wallpapers/everforest.png");
    }

    #[test]
    fn path_expressions_resolve_back_to_the_image_path() {
        let choice = initial_choice(
            "(./wallpapers + \"/my paper.png\")",
            Path::new("/repo/modules/02-core"),
        );
        let WallpaperChoice::File(file) = &choice else {
            panic!("expected a file");
        };
        assert_eq!(
            file.absolute,
            PathBuf::from("/repo/modules/02-core/wallpapers/my paper.png")
        );
        assert_eq!(file.value, "(./wallpapers + \"/my paper.png\")");
    }

    #[test]
    fn quoted_relative_paths_are_rewritten_as_path_expressions() {
        let choice = initial_choice(
            "\"./wallpapers/my paper.png\"",
            Path::new("/repo/modules/02-core"),
        );
        let WallpaperChoice::File(file) = &choice else {
            panic!("expected a file");
        };
        assert_eq!(file.value, "(./wallpapers + \"/my paper.png\")");
    }

    #[test]
    fn preset_apply_literal_uses_the_themed_path() {
        let preset = WallpaperPresetChoice {
            name: "everforest".to_string(),
            label: "Default".to_string(),
            path: PathBuf::from("/nix/store/abc/user/builtin/everforest.png"),
        };
        let ui = WallpaperUi {
            current: WallpaperChoice::Preset(preset),
            tiles: Vec::new(),
            default_path: None,
        };
        assert_eq!(ui.nix_value(), "\"everforest\"");
        assert_eq!(
            ui.apply_literal(),
            "\"/nix/store/abc/user/builtin/everforest.png\""
        );
    }

    #[test]
    fn auto_apply_literal_uses_the_default_themed_path() {
        let ui = WallpaperUi {
            current: WallpaperChoice::Auto,
            tiles: Vec::new(),
            default_path: Some("/nix/store/abc/user/builtin/everforest.png".to_string()),
        };
        assert_eq!(ui.nix_value(), "null");
        assert_eq!(
            ui.apply_literal(),
            "\"/nix/store/abc/user/builtin/everforest.png\""
        );
    }
}
