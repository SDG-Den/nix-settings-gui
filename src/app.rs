use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::config::AppConfig;
use crate::dsl::{SnippetSpec, Visibility, WidgetKind};
use crate::edit;
use crate::metadata::{self, Target};
use crate::model::{InsertMarker, MenuModel, Record};
use crate::nixd::OptionInfo;
use crate::scan::{self, ScanResult};
use crate::widgets::{self, Editor};

pub const APP_ID: &str = "org.nix_settings_gui.NixSettingsGui";

enum Msg {
    Scanned(Result<ScanResult, String>),
    Saved(Result<usize, String>),
    SaveProgress(f64, String),
    Inserted(Result<(), String>),
    SnippetInserted(Result<(), String>),
    Infos(String, Vec<(String, OptionInfo)>),
    Suggestions(Vec<String>),
    Invalid(Vec<String>),
    GitPulled(Result<bool, String>),
    GitSynced(Result<String, String>),
}

type Tx = async_channel::Sender<Msg>;

/// A pending package-search result handler plus its request id, keyed in
/// `PackageService::callbacks`.
type PackageCallback = Box<dyn FnOnce(Vec<String>)>;
type PackageCallbacks = Rc<RefCell<HashMap<u64, PackageCallback>>>;

/// One rendered option in a menu: the record it edits, the live control, and
/// the widgets that let the row be filtered or refined after nixd answers.
struct ControlRow {
    record: Record,
    editor: Rc<RefCell<Editor>>,
    row: gtk::Box,
    slot: gtk::Box,
    desc: gtk::Label,
    dirty: gtk::Label,
    reset: gtk::Button,
    /// Position of this control's record in source order, used to preserve
    /// declaration order when no explicit `order` is set.
    src_index: usize,
    /// Whether this control's `visibleWhen` dependency currently holds.
    dep_visible: Cell<bool>,
}

impl ControlRow {
    fn set_editor(&self, editor: Editor) {
        while let Some(child) = self.slot.first_child() {
            self.slot.remove(&child);
        }
        self.slot.append(&editor.root());
        self.slot.append(&self.reset);
        *self.editor.borrow_mut() = editor;
    }
}

struct MenuPage {
    file: String,
    controls: Vec<Rc<ControlRow>>,
    root: gtk::Widget,
    content: gtk::Box,
    group_headers: Vec<(bool, String, gtk::Label)>,
    group_inserts: Vec<(String, gtk::Button)>,
}

/// An "Add <snippet>" button placed at an `@insert` marker's group.
struct InsertButton {
    group: String,
    order: i64,
    button: gtk::Button,
}

/// A single long-lived nixd process shared by all package searches. Keystrokes
/// are queued to a worker thread; results come back on the main thread keyed by
/// request id so several popovers can search concurrently.
struct PackageService {
    requests: async_channel::Sender<(u64, String, String)>,
    callbacks: PackageCallbacks,
    next_id: Rc<Cell<u64>>,
}

impl PackageService {
    fn new() -> Rc<Self> {
        let (requests, request_rx) = async_channel::bounded::<(u64, String, String)>(16);
        let (result_tx, result_rx) = async_channel::bounded::<(u64, Vec<String>)>(16);
        std::thread::spawn(move || {
            while let Ok((id, flake, term)) = request_rx.recv_blocking() {
                let flake_arg = if flake.is_empty() {
                    None
                } else {
                    Some(flake.as_str())
                };
                let names =
                    crate::nixd::complete_packages(flake_arg, &term, id).unwrap_or_default();
                let _ = result_tx.send_blocking((id, names));
            }
        });

        let callbacks: PackageCallbacks = Rc::new(RefCell::new(HashMap::new()));
        {
            let callbacks = callbacks.clone();
            glib::MainContext::default().spawn_local(async move {
                while let Ok((id, names)) = result_rx.recv().await {
                    if let Some(done) = callbacks.borrow_mut().remove(&id) {
                        done(names);
                    }
                }
            });
        }
        Rc::new(Self {
            requests,
            callbacks,
            next_id: Rc::new(Cell::new(0)),
        })
    }

    fn search(&self, flake: String, term: String, done: PackageCallback) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.callbacks.borrow_mut().insert(id, done);
        if self.requests.try_send((id, flake, term)).is_err() {
            self.callbacks.borrow_mut().remove(&id);
        }
    }
}

struct App {
    window: adw::ApplicationWindow,
    title: gtk::Label,
    flake_combo: gtk::ComboBoxText,
    search: gtk::SearchEntry,
    host_combo: gtk::ComboBoxText,
    system_toggle: gtk::ToggleButton,
    advanced_toggle: gtk::ToggleButton,
    list: gtk::ListBox,
    stack: gtk::Stack,
    nixd_banner: adw::Banner,
    status: gtk::Label,
    spinner: gtk::Spinner,
    progress: gtk::ProgressBar,
    package_service: Rc<PackageService>,
    tx: Tx,

    config: RefCell<AppConfig>,
    base: RefCell<String>,
    scan: RefCell<Option<ScanResult>>,
    targets: RefCell<Vec<String>>,
    target: RefCell<String>,
    hosts: RefCell<Vec<String>>,
    current_host: RefCell<String>,
    current_file: RefCell<String>,

    flakes: RefCell<Vec<PathBuf>>,
    pages: RefCell<Vec<MenuPage>>,
    suggest_store: RefCell<Option<gtk::ListStore>>,
    suggest_parent: RefCell<Option<String>>,
    suggest_cache: RefCell<HashMap<String, Vec<String>>>,
    system_only: Cell<bool>,
    advanced: Cell<bool>,
    manual_host: Cell<bool>,
    pending_rebuild: Cell<bool>,
    watchers: RefCell<Vec<gio::FileMonitor>>,
}

// ---------------------------------------------------------------------------
// UI construction
// ---------------------------------------------------------------------------

fn build_ui(app: &adw::Application, tx: &Tx) -> Rc<App> {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        ".search-hit { outline: 2px solid @accent_color; border-radius: 6px; }\n\
         .group-header { color: @accent_color; font-weight: 700; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let title = gtk::Label::builder()
        .label("nix-settings-gui")
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["title-4"])
        .build();

    let flake_combo = gtk::ComboBoxText::new();
    flake_combo.set_tooltip_text(Some("Flake or config directory to open"));

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search settings")
        .hexpand(true)
        .build();

    let system_toggle = gtk::ToggleButton::builder()
        .label("System only")
        .tooltip_text("Show only the configured host's settings")
        .build();

    let host_combo = gtk::ComboBoxText::new();
    host_combo.set_tooltip_text(Some("Host to edit"));
    host_combo.set_visible(false);

    let advanced_toggle = gtk::ToggleButton::builder()
        .icon_name("preferences-other-symbolic")
        .tooltip_text("Show hidden menus and advanced options")
        .build();

    let title_widget = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    title_widget.append(&title);
    title_widget.append(&flake_combo);
    title_widget.append(&search);
    title_widget.append(&system_toggle);
    title_widget.append(&host_combo);
    title_widget.append(&advanced_toggle);

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title_widget));

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("navigation-sidebar");
    let list_scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();

    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    let nixd_banner = adw::Banner::new("");
    nixd_banner.set_revealed(false);

    let content_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_pane.append(&stack);
    content_pane.append(&nixd_banner);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&list_scroll));
    paned.set_end_child(Some(&content_pane));
    paned.set_position(320);
    paned.set_hexpand(true);
    paned.set_vexpand(true);
    paned.set_shrink_start_child(true);
    paned.set_shrink_end_child(true);

    let status = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["dim-label"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    spinner.set_tooltip_text(Some("Working..."));

    let progress = gtk::ProgressBar::new();
    progress.set_visible(false);
    progress.set_width_request(160);
    progress.set_valign(gtk::Align::Center);

    let configure_btn = gtk::Button::with_label("Configure");
    let add_file_btn = gtk::Button::with_label("Add file");
    let refresh_btn = gtk::Button::builder()
        .label("Reload")
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Reload from disk")
        .build();
    let save_btn = gtk::Button::with_label("Save");
    save_btn.add_css_class("suggested-action");
    let rebuild_btn = gtk::Button::with_label("Save & rebuild");
    let git_btn = gtk::Button::builder()
        .label("Git sync")
        .icon_name("emblem-synchronizing-symbolic")
        .tooltip_text("Pull, commit local changes, and push")
        .build();
    let info_btn = gtk::Button::builder()
        .label("Query info")
        .icon_name("dialog-information-symbolic")
        .tooltip_text("Query nixd for types and descriptions of this menu")
        .build();

    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    menu_box.set_margin_top(6);
    menu_box.set_margin_bottom(6);
    menu_box.set_margin_start(6);
    menu_box.set_margin_end(6);
    for button in [&configure_btn, &add_file_btn, &refresh_btn, &info_btn] {
        button.set_halign(gtk::Align::Fill);
        menu_box.append(button);
    }
    let menu_popover = gtk::Popover::new();
    menu_popover.set_child(Some(&menu_box));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("More actions")
        .build();
    menu_button.set_popover(Some(&menu_popover));
    for button in [&configure_btn, &add_file_btn, &refresh_btn, &info_btn] {
        let popover = menu_popover.clone();
        button.connect_clicked(move |_| popover.popdown());
    }

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bottom.set_margin_top(6);
    bottom.set_margin_bottom(6);
    bottom.set_margin_start(8);
    bottom.set_margin_end(8);
    bottom.append(&menu_button);
    bottom.append(&spinner);
    bottom.append(&status);
    bottom.append(&progress);
    bottom.append(&save_btn);
    bottom.append(&rebuild_btn);
    bottom.append(&git_btn);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&paned);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&body));
    toolbar_view.add_bottom_bar(&bottom);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("nix-settings-gui")
        .default_width(1180)
        .default_height(760)
        .content(&toolbar_view)
        .build();

    let app_state = Rc::new(App {
        window,
        title,
        flake_combo,
        search,
        host_combo,
        system_toggle,
        advanced_toggle,
        list,
        stack,
        nixd_banner,
        status,
        spinner,
        progress,
        package_service: PackageService::new(),
        tx: tx.clone(),
        config: RefCell::new(AppConfig::load()),
        base: RefCell::new(String::new()),
        scan: RefCell::new(None),
        targets: RefCell::new(Vec::new()),
        target: RefCell::new(String::new()),
        hosts: RefCell::new(Vec::new()),
        current_host: RefCell::new(String::new()),
        current_file: RefCell::new(String::new()),
        flakes: RefCell::new(Vec::new()),
        pages: RefCell::new(Vec::new()),
        suggest_store: RefCell::new(None),
        suggest_parent: RefCell::new(None),
        suggest_cache: RefCell::new(HashMap::new()),
        system_only: Cell::new(false),
        advanced: Cell::new(false),
        manual_host: Cell::new(false),
        pending_rebuild: Cell::new(false),
        watchers: RefCell::new(Vec::new()),
    });

    wire(
        &app_state,
        tx,
        &configure_btn,
        &add_file_btn,
        &refresh_btn,
        &save_btn,
        &rebuild_btn,
        &git_btn,
        &info_btn,
    );
    install_actions(&app_state);
    {
        let app = app_state.clone();
        app_state
            .window
            .clone()
            .connect_close_request(move |window| {
                if collect_edits(&app).is_empty() {
                    glib::Propagation::Proceed
                } else {
                    let window = window.clone();
                    run_changes_prompt(&app, Box::new(move || window.destroy()));
                    glib::Propagation::Stop
                }
            });
    }
    {
        app_state.window.clone().connect_destroy(|_| {
            crate::nixd::shutdown_nixd();
        });
    }
    app_state
}

#[allow(clippy::too_many_arguments)]
fn wire(
    app: &Rc<App>,
    tx: &Tx,
    configure_btn: &gtk::Button,
    add_file_btn: &gtk::Button,
    refresh_btn: &gtk::Button,
    save_btn: &gtk::Button,
    rebuild_btn: &gtk::Button,
    git_btn: &gtk::Button,
    info_btn: &gtk::Button,
) {
    {
        let app = app.clone();
        let tx = tx.clone();
        app.flake_combo.clone().connect_changed(move |combo| {
            if let Some(index) = combo.active() {
                let path = app
                    .flakes
                    .borrow()
                    .get(index as usize)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                if !path.is_empty() {
                    combo.set_tooltip_text(Some(&path));
                    open_flake(&app, &tx, &path);
                }
            }
        });
    }
    {
        let app = app.clone();
        app.host_combo.clone().connect_changed(move |combo| {
            app.manual_host.set(true);
            if let Some(host) = combo.active_text() {
                *app.current_host.borrow_mut() = host.to_string();
                rebuild_view(&app);
            }
        });
    }
    {
        let app = app.clone();
        app.system_toggle.clone().connect_toggled(move |btn| {
            app.system_only.set(btn.is_active());
            app.host_combo.set_visible(btn.is_active());
            rebuild_view(&app);
        });
    }
    {
        let app = app.clone();
        app.advanced_toggle.clone().connect_toggled(move |btn| {
            app.advanced.set(btn.is_active());
            rebuild_view(&app);
        });
    }
    {
        let app = app.clone();
        app.search.clone().connect_search_changed(move |_| {
            rebuild_menu_list(&app);
            apply_control_filter(&app);
        });
    }
    {
        let app = app.clone();
        app.list.clone().connect_row_selected(move |_, row| {
            let Some(row) = row else {
                return;
            };
            let name = row.widget_name();
            if name.is_empty() {
                return;
            }
            app.stack.set_visible_child_name(name.as_str());
            *app.current_file.borrow_mut() = name.to_string();
            apply_control_filter(&app);
            query_current_page(&app, &app.tx);
        });
    }
    {
        let app = app.clone();
        configure_btn
            .clone()
            .connect_clicked(move |_| open_configure(&app));
    }
    {
        let app = app.clone();
        let tx = tx.clone();
        add_file_btn
            .clone()
            .connect_clicked(move |_| open_add_file(&app, &tx));
    }
    {
        let app = app.clone();
        let tx = tx.clone();
        refresh_btn
            .clone()
            .connect_clicked(move |_| reload(&app, &tx));
    }
    {
        let app = app.clone();
        let tx = tx.clone();
        save_btn
            .clone()
            .connect_clicked(move |_| save_all(&app, &tx, false));
    }
    {
        let app = app.clone();
        let tx = tx.clone();
        rebuild_btn
            .clone()
            .connect_clicked(move |_| save_all(&app, &tx, true));
    }
    {
        let app = app.clone();
        let tx = tx.clone();
        git_btn
            .clone()
            .connect_clicked(move |_| start_git_sync(&app, &tx));
    }
    {
        let app = app.clone();
        let tx = tx.clone();
        info_btn
            .clone()
            .connect_clicked(move |_| query_current_page(&app, &tx));
    }
}

/// Register the window actions named in the design's shortcut table and bind
/// them to accelers on the application.
fn install_actions(app: &Rc<App>) {
    let tx = app.tx.clone();

    {
        let window = app.window.clone();
        let app = app.clone();
        let tx = tx.clone();
        let action = gio::SimpleAction::new("save", None);
        action.connect_activate(move |_, _| save_all(&app, &tx, false));
        window.add_action(&action);
    }
    {
        let window = app.window.clone();
        let app = app.clone();
        let tx = tx.clone();
        let action = gio::SimpleAction::new("save-rebuild", None);
        action.connect_activate(move |_, _| save_all(&app, &tx, true));
        window.add_action(&action);
    }
    {
        let window = app.window.clone();
        let app = app.clone();
        let tx = tx.clone();
        let action = gio::SimpleAction::new("reload", None);
        action.connect_activate(move |_, _| reload(&app, &tx));
        window.add_action(&action);
    }
    {
        let window = app.window.clone();
        let app = app.clone();
        let action = gio::SimpleAction::new("focus-search", None);
        action.connect_activate(move |_, _| {
            app.search.grab_focus();
        });
        window.add_action(&action);
    }
    {
        let window = app.window.clone();
        let app = app.clone();
        let action = gio::SimpleAction::new("discard", None);
        action.connect_activate(move |_, _| discard_changes(&app));
        window.add_action(&action);
    }
    {
        let window = app.window.clone();
        let app = app.clone();
        let tx = tx.clone();
        let action = gio::SimpleAction::new("preview", None);
        action.connect_activate(move |_, _| preview_changes(&app, &tx));
        window.add_action(&action);
    }
    {
        let window = app.window.clone();
        let action = gio::SimpleAction::new("close", None);
        action.connect_activate(move |_, _| window.close());
        app.window.add_action(&action);
    }

    if let Some(application) = app.window.application() {
        application.set_accels_for_action("win.save", &["<Primary>s"]);
        application.set_accels_for_action("win.save-rebuild", &["<Primary><Shift>s"]);
        application.set_accels_for_action("win.reload", &["<Primary>r"]);
        application.set_accels_for_action("win.focus-search", &["<Primary>f"]);
        application.set_accels_for_action("win.discard", &["<Primary><Shift>r"]);
        application.set_accels_for_action("win.preview", &["<Primary>d"]);
        application.set_accels_for_action("win.close", &["Escape"]);
        application.set_accels_for_action("app.quit", &["<Primary>q"]);
    }
}

// ---------------------------------------------------------------------------
// scan and view
// ---------------------------------------------------------------------------

fn open_flake(app: &Rc<App>, tx: &Tx, path: &str) {
    *app.base.borrow_mut() = path.to_string();
    app.config.borrow_mut().remember(path);
    let _ = app.config.borrow().save();
    app.status.set_text(&format!("Scanning {path}..."));
    crate::nixd::warm(Some(path));
    let path = path.to_string();
    spawn(tx, move |tx| {
        let result = scan::scan_root(&path);
        let _ = tx.send_blocking(Msg::Scanned(result));
    });
}

fn rebuild_view(app: &Rc<App>) {
    rebuild_menu_list(app);
    rebuild_pages(app);
}

fn visible_menus(app: &Rc<App>) -> Vec<crate::model::MenuModel> {
    let scan = app.scan.borrow();
    let Some(scan) = scan.as_ref() else {
        return Vec::new();
    };
    let host = app.current_host.borrow().clone();
    let menus: Vec<&crate::model::MenuModel> = if app.system_only.get() {
        scan.menus_for(&host)
    } else {
        scan.menus.iter().collect()
    };
    menus
        .into_iter()
        .filter(|menu| menu.has_menu)
        .filter(|menu| app.advanced.get() || !menu.spec.hidden)
        .cloned()
        .collect()
}

fn menu_matches(menu: &crate::model::MenuModel, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if menu.display_name().to_lowercase().contains(query)
        || menu.file.to_lowercase().contains(query)
    {
        return true;
    }
    menu.records.iter().any(|record| {
        record.path().to_lowercase().contains(query)
            || record
                .spec
                .label
                .as_deref()
                .map(|l| l.to_lowercase().contains(query))
                .unwrap_or(false)
    })
}

const FALLBACK_MENU_ICON: &str = "preferences-system-symbolic";

fn pick_icon_name(has_icon: impl Fn(&str) -> bool, icon: Option<&str>) -> Option<String> {
    let name = icon?;
    if has_icon(name) {
        return Some(name.to_string());
    }
    if has_icon(FALLBACK_MENU_ICON) {
        return Some(FALLBACK_MENU_ICON.to_string());
    }
    None
}

fn resolve_menu_icon(icon: Option<&str>) -> Option<String> {
    let display = gtk::gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    pick_icon_name(|name| theme.has_icon(name), icon)
}

fn rebuild_menu_list(app: &Rc<App>) {
    while let Some(child) = app.list.first_child() {
        app.list.remove(&child);
    }
    let query = app.search.text().to_lowercase();
    let mut menus = visible_menus(app);
    menus.sort_by(|a, b| {
        let ac = a.spec.category.clone().unwrap_or_default();
        let bc = b.spec.category.clone().unwrap_or_default();
        ac.cmp(&bc)
            .then_with(|| a.order().cmp(&b.order()))
            .then_with(|| a.display_name().cmp(&b.display_name()))
    });
    let mut last_category: Option<String> = None;
    for menu in &menus {
        if !menu_matches(menu, &query) {
            continue;
        }
        let category = menu.spec.category.clone().unwrap_or_default();
        if last_category.as_deref() != Some(category.as_str()) {
            last_category = Some(category.clone());
            if !category.is_empty() {
                let header = gtk::Label::builder()
                    .label(&category)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .margin_top(6)
                    .css_classes(["group-header"])
                    .build();
                let row = gtk::ListBoxRow::builder()
                    .activatable(false)
                    .selectable(false)
                    .child(&header)
                    .build();
                app.list.append(&row);
            }
        }
        let name = menu.display_name();
        let label = gtk::Label::builder()
            .label(&name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&menu.file);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let Some(icon) = resolve_menu_icon(menu.spec.icon.as_deref()) {
            content.append(&gtk::Image::from_icon_name(icon.as_str()));
        }
        content.append(&label);
        row.set_child(Some(&content));
        app.list.append(&row);
    }

    let current = app.current_file.borrow().clone();
    select_file(app, &current);
}

fn select_file(app: &Rc<App>, file: &str) {
    let mut child = app.list.first_child();
    let mut first_selectable: Option<gtk::ListBoxRow> = None;
    while let Some(widget) = child {
        if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() {
            if !row.widget_name().is_empty() && first_selectable.is_none() {
                first_selectable = Some(row.clone());
            }
            if row.widget_name() == file {
                app.list.select_row(Some(row));
                return;
            }
        }
        child = widget.next_sibling();
    }
    if let Some(row) = first_selectable {
        app.list.select_row(Some(&row));
    }
}

fn rebuild_pages(app: &Rc<App>) {
    for page in app.pages.borrow().iter() {
        app.stack.remove(&page.root);
    }
    app.pages.borrow_mut().clear();

    let menus = visible_menus(app);
    let mut pages = Vec::new();
    for menu in &menus {
        let page = build_page(app, menu);
        app.stack.add_named(&page.root, Some(&menu.file));
        pages.push(page);
    }
    *app.pages.borrow_mut() = pages;

    let current = app.current_file.borrow().clone();
    if app.pages.borrow().iter().any(|p| p.file == current) {
        app.stack.set_visible_child_name(&current);
    } else if let Some(first) = app.pages.borrow().first() {
        let file = first.file.clone();
        app.stack.set_visible_child_name(&file);
        *app.current_file.borrow_mut() = file;
    }
    apply_control_filter(app);
    query_current_page(app, &app.tx);
}

fn build_page(app: &Rc<App>, menu: &crate::model::MenuModel) -> MenuPage {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(16);
    content.set_margin_end(16);

    let heading = gtk::Label::builder()
        .label(menu.display_name())
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-2"])
        .build();
    content.append(&heading);
    if let Some(description) = &menu.spec.description {
        let subtitle = gtk::Label::builder()
            .label(description)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build();
        content.append(&subtitle);
    }

    let mut ordinary = Vec::new();
    let mut advanced = Vec::new();
    let ranks = crate::model::group_ranks(&menu.records);
    for (src_index, record) in menu.records.iter().enumerate() {
        if record.spec.hidden && !app.advanced.get() {
            continue;
        }
        let row = build_control_row(app, record, src_index);
        if record.spec.advanced {
            advanced.push(row);
        } else {
            ordinary.push(row);
        }
    }

    ordinary.sort_by(|a, b| control_cmp(a, b, &ranks));
    advanced.sort_by(|a, b| control_cmp(a, b, &ranks));

    let mut inserts = build_insert_buttons(app, menu);
    let mut headers = Vec::new();
    let mut group_inserts: Vec<(String, gtk::Button)> = Vec::new();
    add_grouped(
        &content,
        &ordinary,
        &mut inserts,
        false,
        &mut headers,
        &mut group_inserts,
    );
    if !advanced.is_empty() {
        let expander = gtk::Expander::new(Some("Advanced"));
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 12);
        inner.set_margin_top(6);
        let mut no_inserts = Vec::new();
        add_grouped(
            &inner,
            &advanced,
            &mut no_inserts,
            true,
            &mut headers,
            &mut group_inserts,
        );
        expander.set_child(Some(&inner));
        content.append(&expander);
    }

    inserts.sort_by(|a, b| a.group.cmp(&b.group).then_with(|| a.order.cmp(&b.order)));
    for insert in inserts {
        content.append(&insert.button);
    }

    let add = gtk::Button::with_label("Add option");
    add.add_css_class("pill");
    add.set_halign(gtk::Align::Start);
    {
        let app = app.clone();
        let file = menu.file.clone();
        let origin = menu
            .records
            .first()
            .map(|r| r.origin.clone())
            .unwrap_or_default();
        add.connect_clicked(move |_| {
            open_add_option(&app, &file, &origin);
        });
    }
    content.append(&add);

    content.append(&gtk::Box::new(gtk::Orientation::Vertical, 0));

    let scrolled = gtk::ScrolledWindow::builder()
        .child(&content)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_width(false)
        .vexpand(true)
        .hexpand(true)
        .build();

    let mut controls: Vec<Rc<ControlRow>> = ordinary;
    controls.extend(advanced);
    evaluate_dependencies(&controls);

    MenuPage {
        file: menu.file.clone(),
        controls,
        root: scrolled.upcast(),
        content,
        group_headers: headers,
        group_inserts,
    }
}

fn add_grouped(
    content: &gtk::Box,
    controls: &[Rc<ControlRow>],
    inserts: &mut Vec<InsertButton>,
    advanced: bool,
    headers: &mut Vec<(bool, String, gtk::Label)>,
    group_inserts: &mut Vec<(String, gtk::Button)>,
) {
    let mut last_group: Option<String> = None;
    for control in controls {
        let group = control.record.spec.group.clone().unwrap_or_default();
        if last_group.as_deref() != Some(group.as_str()) {
            if let Some(previous) = last_group.take() {
                flush_inserts(content, inserts, &previous, group_inserts);
            }
            if !group.is_empty() {
                let label = gtk::Label::builder()
                    .label(&group)
                    .xalign(0.0)
                    .margin_top(6)
                    .css_classes(["group-header"])
                    .build();
                content.append(&label);
                headers.push((advanced, group.clone(), label.clone()));
            }
            last_group = Some(group);
        }
        content.append(&control.row);
    }
    if let Some(previous) = last_group.take() {
        flush_inserts(content, inserts, &previous, group_inserts);
    }
}

/// Append the insert buttons assigned to `group`, in `order`, then drop them so
/// they are not placed again.
fn flush_inserts(
    content: &gtk::Box,
    inserts: &mut Vec<InsertButton>,
    group: &str,
    group_inserts: &mut Vec<(String, gtk::Button)>,
) {
    inserts.sort_by_key(|insert| insert.order);
    let mut index = 0;
    while index < inserts.len() {
        if inserts[index].group == group {
            let insert = inserts.remove(index);
            group_inserts.push((group.to_string(), insert.button.clone()));
            content.append(&insert.button);
        } else {
            index += 1;
        }
    }
}

/// Drop a leading `Add`/`add` from a snippet label so the button prefix does
/// not double it when the label already reads "Add ...".
fn strip_add_prefix(label: &str) -> String {
    let trimmed = label.trim();
    let lower = trimmed.to_lowercase();
    if lower == "add" {
        String::new()
    } else if lower.starts_with("add ") {
        trimmed[4..].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Build the "Add <snippet>" button for every `@insert` marker, resolving its
/// target group from the marker, or from the record directly above it.
fn build_insert_buttons(app: &Rc<App>, menu: &MenuModel) -> Vec<InsertButton> {
    let mut out = Vec::new();
    for marker in &menu.inserts {
        let Some(snippet) = menu.snippets.iter().find(|s| s.name == marker.name) else {
            continue;
        };
        let label = marker
            .label
            .clone()
            .or_else(|| (!snippet.label.is_empty()).then(|| snippet.label.clone()))
            .unwrap_or_else(|| marker.name.clone());
        let label = strip_add_prefix(&label);
        let text = if label.is_empty() {
            "Add".to_string()
        } else {
            format!("Add {label}")
        };
        let button = gtk::Button::with_label(&text);
        button.add_css_class("pill");
        button.set_halign(gtk::Align::Start);
        {
            let app = app.clone();
            let file = menu.file.clone();
            let marker = marker.clone();
            let snippet = snippet.clone();
            button.connect_clicked(move |_| {
                open_snippet_dialog(&app, &file, &marker, &snippet);
            });
        }
        let group = marker
            .group
            .clone()
            .unwrap_or_else(|| preceding_group(menu, marker.offset));
        out.push(InsertButton {
            group,
            order: marker.order.unwrap_or(1000),
            button,
        });
    }
    out
}

/// The group of the record directly above `offset` in the file, or empty when
/// nothing precedes the marker.
fn preceding_group(menu: &MenuModel, offset: usize) -> String {
    menu.records
        .iter()
        .filter(|record| record.byte_start < offset)
        .max_by_key(|record| record.byte_start)
        .map(|record| record.spec.group.clone().unwrap_or_default())
        .unwrap_or_default()
}

fn control_cmp(
    a: &ControlRow,
    b: &ControlRow,
    ranks: &HashMap<String, usize>,
) -> std::cmp::Ordering {
    crate::model::record_order(&a.record, a.src_index, &b.record, b.src_index, ranks)
}

/// A control stays inline (label left, widget right) only for a `switch`, a
/// `spin`, and a `dropdown` whose longest choice is short. Everything else
/// stacks the widget below the label and fills the width.
fn control_is_inline(editor: &Editor) -> bool {
    match editor {
        Editor::Switch(_) | Editor::Spin(_) => true,
        Editor::Dropdown { choices, .. } => {
            choices.iter().map(|c| c.chars().count()).max().unwrap_or(0) < 24
        }
        Editor::Text(_)
        | Editor::Path(_)
        | Editor::Multiline { .. }
        | Editor::Package { .. } => false,
        _ => false,
    }
}

/// The flake's available module names per kind, collected from scan origins.
fn build_context(app: &Rc<App>) -> widgets::BuildContext {
    let mut ctx = widgets::BuildContext::default();
    {
        let scan = app.scan.borrow();
        if let Some(scan) = scan.as_ref() {
            for origin in scan.origin_menu.keys() {
                if let Some(name) = origin.strip_prefix("modules.nixos.") {
                    push_unique(&mut ctx.nixos_modules, name);
                } else if let Some(name) = origin.strip_prefix("modules.homeManager.") {
                    push_unique(&mut ctx.hm_modules, name);
                }
            }
            let mut presets = HashMap::new();
            for menu in &scan.menus {
                if !menu.widget_presets.is_empty() {
                    presets.insert(menu.file.clone(), menu.widget_presets.clone());
                }
            }
            ctx.presets = std::rc::Rc::new(presets);
        }
    }
    ctx.nixos_modules.sort();
    ctx.hm_modules.sort();
    ctx.search = Some(package_search(app));
    ctx
}

/// Package search hook: queries nixd off the main thread and delivers the
/// results back on the main thread.
fn package_search(app: &Rc<App>) -> widgets::PackageSearch {
    let service = app.package_service.clone();
    let flake = app.config.borrow().flake_path.clone().unwrap_or_default();
    Rc::new(move |term: String, done: Box<dyn FnOnce(Vec<String>)>| {
        service.search(flake.clone(), term, done);
    })
}

fn push_unique(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|n| n == name) {
        list.push(name.to_string());
    }
}

fn build_control_row(app: &Rc<App>, record: &Record, src_index: usize) -> Rc<ControlRow> {
    let mut ctx = build_context(app);
    ctx.advanced = app.advanced.get();
    let editor = widgets::build(record, None, &ctx);
    let inline = control_is_inline(&editor);
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row.set_margin_top(4);
    row.set_margin_bottom(4);

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let label_text = record
        .spec
        .label
        .clone()
        .or_else(|| record.segments.last().cloned())
        .unwrap_or_else(|| record.path());
    let name = gtk::Label::builder()
        .label(label_text)
        .xalign(0.0)
        .wrap(true)
        .build();
    labels.append(&name);

    let desc = gtk::Label::builder()
        .label(record.spec.description.clone().unwrap_or_default())
        .xalign(0.0)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["dim-label", "caption"])
        .build();
    desc.set_visible(!desc.label().is_empty());

    let dirty = gtk::Label::builder()
        .label("•")
        .xalign(0.0)
        .visible(false)
        .css_classes(["dim-label"])
        .build();
    labels.append(&dirty);

    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    slot.set_valign(gtk::Align::Center);
    if inline {
        slot.set_halign(gtk::Align::End);
    } else {
        slot.set_hexpand(true);
        slot.set_halign(gtk::Align::Fill);
    }

    let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
    reset.add_css_class("flat");
    reset.set_tooltip_text(Some("Reset to default"));
    reset.set_valign(gtk::Align::Center);
    reset.set_halign(gtk::Align::End);

    slot.append(&editor.root());
    slot.append(&reset);

    if inline {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.append(&labels);
        line.append(&slot);
        row.append(&line);
    } else {
        row.append(&labels);
        row.append(&slot);
    }
    row.append(&desc);

    if !record.origin.is_empty() {
        let origin = gtk::Label::builder()
            .label(format!("imported from {}", record.origin))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .build();
        row.append(&origin);
    }

    row.set_tooltip_text(Some(&record.path()));

    let control = Rc::new(ControlRow {
        record: record.clone(),
        editor: Rc::new(RefCell::new(editor)),
        row,
        slot,
        desc,
        dirty,
        reset,
        src_index,
        dep_visible: Cell::new(true),
    });

    {
        let app = app.clone();
        let weak = Rc::downgrade(&control);
        control.reset.clone().connect_clicked(move |_| {
            if let Some(control) = weak.upgrade() {
                reset_to_default(&app, &control);
            }
        });
    }

    connect_dirty(app, &control);
    control
}

/// Set the dirty bullet whenever any editor widget changes, based on whether
/// the editor still matches the record's original value, and re-evaluate any
/// visibility dependencies that key off the changed editor.
fn connect_dirty(app: &Rc<App>, row: &ControlRow) {
    let dirty = row.dirty.clone();
    let original = row.record.value.clone();
    let weak = Rc::downgrade(&row.editor);
    let app = app.clone();
    let update: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(editor) = weak.upgrade() {
            let unchanged = editor.borrow().unchanged(&original);
            dirty.set_visible(!unchanged);
        }
        refresh_dependencies(&app);
    });
    connect_editor_signals(&row.editor.borrow(), &update);
}

/// Connect `update` to the change signal of every widget owned by the editor,
/// recursing into submodule children.
fn connect_editor_signals(editor: &Editor, update: &Rc<dyn Fn()>) {
    editor.connect_changed(update);
}

/// Recompute `dep_visible` for every control from the current values of the
/// options it depends on, then re-apply the combined visibility. Dependency
/// scope is one page: `visibleWhen.path` is looked up by full option path
/// within the same menu.
fn refresh_dependencies(app: &Rc<App>) {
    let current = app.current_file.borrow().clone();
    for page in app.pages.borrow().iter() {
        if page.file == current {
            evaluate_dependencies(&page.controls);
        }
    }
    apply_control_filter(app);
}

/// Current value of `path` in `values`, tolerating the `config.` prefix the
/// scanner records for annotated assignments inside a module's `config = { }`
/// wrapper: annotation paths omit that wrapper, so both spellings are tried.
fn resolve_dependency_value<'a>(
    values: &'a HashMap<String, String>,
    path: &str,
) -> Option<&'a String> {
    values
        .get(path)
        .or_else(|| values.get(&format!("config.{path}")))
        .or_else(|| path.strip_prefix("config.").and_then(|s| values.get(s)))
}

/// Whether `vis` holds for the governing value found in `values`.
fn dependency_visible(vis: &Visibility, values: &HashMap<String, String>) -> bool {
    let current = resolve_dependency_value(values, &vis.path).map(String::as_str);
    vis.holds(current)
}

/// Set each control's `dep_visible` from its `visibleWhen` condition, looking
/// the governing value up through `resolve_dependency_value`. A governing
/// option that is absent fails open, so a stale path never hides a setting.
fn evaluate_dependencies(controls: &[Rc<ControlRow>]) {
    let mut values: HashMap<String, String> = HashMap::new();
    for control in controls {
        values
            .entry(control.record.path())
            .or_insert_with(|| control.editor.borrow().to_nix());
    }
    for control in controls {
        let visible = match &control.record.spec.visible_when {
            Some(vis) => dependency_visible(vis, &values),
            None => true,
        };
        control.dep_visible.set(visible);
    }
}

fn visible_group_names(
    rows: impl IntoIterator<Item = ((bool, String), bool)>,
) -> HashSet<(bool, String)> {
    let mut set = HashSet::new();
    for ((advanced, group), row_visible) in rows {
        if row_visible && !group.is_empty() {
            set.insert((advanced, group));
        }
    }
    set
}

fn apply_control_filter(app: &Rc<App>) {
    let query = app.search.text().to_lowercase();
    let current = app.current_file.borrow().clone();
    for page in app.pages.borrow().iter() {
        if page.file != current {
            continue;
        }
        let mut first_match: Option<&Rc<ControlRow>> = None;
        let mut visible_rows: Vec<((bool, String), bool)> = Vec::new();
        for control in &page.controls {
            control.row.remove_css_class("search-hit");
            let matches = query.is_empty()
                || control.record.path().to_lowercase().contains(&query)
                || control
                    .record
                    .spec
                    .label
                    .as_deref()
                    .map(|l| l.to_lowercase().contains(&query))
                    .unwrap_or(false);
            let visible = matches && control.dep_visible.get();
            control.row.set_visible(visible);
            if visible && !query.is_empty() && first_match.is_none() {
                first_match = Some(control);
            }
            let advanced = control.record.spec.advanced;
            let group = control.record.spec.group.clone().unwrap_or_default();
            visible_rows.push(((advanced, group), visible));
        }
        let visible_groups = visible_group_names(visible_rows);
        for (advanced, group_name, label) in &page.group_headers {
            let visible = visible_groups.contains(&(*advanced, group_name.clone()));
            label.set_visible(visible);
        }
        for (group_name, button) in &page.group_inserts {
            let has_header = !group_name.is_empty()
                && page
                    .group_headers
                    .iter()
                    .any(|(adv, name, _)| !*adv && name == group_name);
            if has_header {
                let visible = visible_groups.contains(&(false, group_name.clone()));
                button.set_visible(visible);
            }
        }
        if let Some(control) = first_match {
            control.row.add_css_class("search-hit");
            scroll_to_control(page, &control.row);
        }
    }
}

/// Scroll the page so `row` sits just below the top of the viewport.
fn scroll_to_control(page: &MenuPage, row: &gtk::Box) {
    let Ok(scrolled) = page.root.clone().downcast::<gtk::ScrolledWindow>() else {
        return;
    };
    if let Some(bounds) = row.compute_bounds(&page.content) {
        scrolled
            .vadjustment()
            .set_value(scroll_offset(bounds.y() as f64));
    }
}

fn scroll_offset(bounds_y: f64) -> f64 {
    bounds_y - 12.0
}

/// Rebuild one control's editor from the option registry's default value.
fn reset_to_default(app: &Rc<App>, control: &ControlRow) {
    let Some(target) = Target::parse(&app.target.borrow()) else {
        app.status.set_text("No option target for this config");
        return;
    };
    let base = app.base.borrow().clone();
    let path = control.record.segments.clone();
    match metadata::default_expression(&base, &target, &path) {
        Ok(default) if !default.trim().is_empty() => {
            let mut record = control.record.clone();
            record.value = default;
            let editor = widgets::build(&record, None, &build_context(app));
            control.set_editor(editor);
            connect_dirty(app, control);
            app.status.set_text("Reset to default");
        }
        Ok(_) => app.status.set_text("No default available"),
        Err(e) => app.status.set_text(&format!("Cannot resolve default: {e}")),
    }
}

// ---------------------------------------------------------------------------
// save and rebuild
// ---------------------------------------------------------------------------

/// Every staged edit across every page, in page order. Controls hidden by a
/// `visibleWhen` dependency are skipped, so a disabled parent cannot commit
/// its children's staged edits.
fn collect_edits(app: &Rc<App>) -> Vec<edit::PendingEdit> {
    let mut edits = Vec::new();
    for page in app.pages.borrow().iter() {
        for control in &page.controls {
            if !control.dep_visible.get() {
                continue;
            }
            let editor = control.editor.borrow();
            edits.extend(editor.edits(&control.record));
        }
    }
    edits
}

/// Package assignments that changed, as `(path, literal)` pairs to validate
/// with nixd before writing. Hidden-by-dependency controls are skipped.
fn collect_package_checks(app: &Rc<App>) -> Vec<(String, String)> {
    let mut checks = Vec::new();
    for page in app.pages.borrow().iter() {
        for control in &page.controls {
            if !control.dep_visible.get() {
                continue;
            }
            if !widgets::is_package(&control.record) {
                continue;
            }
            let editor = control.editor.borrow();
            if editor.unchanged(&control.record.value) {
                continue;
            }
            checks.push((
                control.record.path(),
                format!("with pkgs; {}", editor.to_nix()),
            ));
        }
    }
    checks
}

fn save_all(app: &Rc<App>, tx: &Tx, rebuild: bool) {
    let edits = collect_edits(app);
    if edits.is_empty() {
        clear_busy(app);
        app.status.set_text("Nothing to save");
        if rebuild {
            run_rebuild(app);
        }
        return;
    }
    let root = app.base.borrow().clone();
    let checks = collect_package_checks(app);
    let flake = app.config.borrow().flake_path.clone();
    set_busy(app, "Saving...", 0.0);
    spawn(tx, move |tx| {
        const VALIDATE_END: f64 = 0.7;
        let report_tx = tx.clone();
        let report = move |done: usize, total: usize| {
            let fraction = if total == 0 {
                VALIDATE_END
            } else {
                VALIDATE_END * done as f64 / total as f64
            };
            let label = if done == 0 {
                "Validating package changes...".to_string()
            } else {
                format!("Validated {done} of {total} package change(s)...")
            };
            let _ = report_tx.send_blocking(Msg::SaveProgress(fraction, label));
        };
        if let Ok(errors) = crate::nixd::query_errors(flake.as_deref(), checks, report) {
            if !errors.is_empty() {
                let _ = tx.send_blocking(Msg::Invalid(errors));
                return;
            }
        }
        let _ = tx.send_blocking(Msg::SaveProgress(
            VALIDATE_END,
            "Writing changes...".to_string(),
        ));
        let result = edit::apply(&edits, &root);
        let _ = tx.send_blocking(Msg::SaveProgress(1.0, "Saved".to_string()));
        let _ = tx.send_blocking(Msg::Saved(result));
    });
    app.pending_rebuild.set(rebuild);
}

/// Show the busy indicators (spinner and progress bar) with a status message.
fn set_busy(app: &Rc<App>, message: &str, fraction: f64) {
    app.spinner.set_visible(true);
    app.spinner.start();
    app.progress.set_visible(true);
    app.progress.set_fraction(fraction);
    app.status.set_text(message);
}

/// Hide the busy indicators once a long-running task finishes.
fn clear_busy(app: &Rc<App>) {
    app.spinner.stop();
    app.spinner.set_visible(false);
    app.progress.set_visible(false);
    app.progress.set_fraction(0.0);
}

/// What the unsaved-changes dialog should do for a response.
#[derive(Debug, PartialEq, Eq)]
enum ChangesAction {
    Save,
    Discard,
    Cancel,
}

fn changes_action(response: gtk::ResponseType) -> ChangesAction {
    match response {
        gtk::ResponseType::Yes => ChangesAction::Save,
        gtk::ResponseType::No => ChangesAction::Discard,
        _ => ChangesAction::Cancel,
    }
}

/// Prompt to save, discard, or cancel staged edits. `on_save` runs after a
/// successful save or an explicit discard, never after cancel.
fn run_changes_prompt(app: &Rc<App>, on_save: Box<dyn Fn()>) {
    let dialog = gtk::Dialog::builder()
        .title("Unsaved changes")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Discard", gtk::ResponseType::No);
    let save = dialog.add_button("Save", gtk::ResponseType::Yes);
    save.add_css_class("suggested-action");
    dialog.set_default_response(gtk::ResponseType::Yes);

    let message = gtk::Label::builder()
        .label("You have unsaved changes. Save them before closing?")
        .xalign(0.0)
        .wrap(true)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    dialog.content_area().append(&message);

    let on_save: Rc<dyn Fn()> = Rc::from(on_save);
    let state = app.clone();
    dialog.connect_response(move |dialog, response| {
        match changes_action(response) {
            ChangesAction::Save => {
                let edits = collect_edits(&state);
                let root = state.base.borrow().clone();
                match edit::apply(&edits, &root) {
                    Ok(count) => {
                        state.status.set_text(&format!("Saved {count} file(s)"));
                        let base = state.base.borrow().clone();
                        spawn(&state.tx, move |tx| {
                            let result = scan::scan_root(&base);
                            let _ = tx.send_blocking(Msg::Scanned(result));
                        });
                        on_save();
                    }
                    Err(e) => {
                        state.status.set_text("Save failed");
                        show_error(&state.window, &e);
                    }
                }
            }
            ChangesAction::Discard => {
                discard_changes(&state);
                on_save();
            }
            ChangesAction::Cancel => {}
        }
        dialog.close();
    });
    dialog.present();
}

/// Drop every staged edit by rebuilding the editors from the scanned records.
fn discard_changes(app: &Rc<App>) {
    rebuild_view(app);
    app.status.set_text("Changes discarded");
}

fn reload(app: &Rc<App>, tx: &Tx) {
    if collect_edits(app).is_empty() {
        rescan(app, tx);
        return;
    }
    let app_for_save = app.clone();
    let tx_for_save = tx.clone();
    run_changes_prompt(app, Box::new(move || rescan(&app_for_save, &tx_for_save)));
}

/// Re-read every file under the current root, exactly like `open_flake`.
fn rescan(app: &Rc<App>, tx: &Tx) {
    let base = app.base.borrow().clone();
    app.status.set_text("Scanning...");
    spawn(tx, move |tx| {
        let result = scan::scan_root(&base);
        let _ = tx.send_blocking(Msg::Scanned(result));
    });
}

/// Show the pending span patches as a unified diff, old text read from disk.
fn preview_changes(app: &Rc<App>, _tx: &Tx) {
    let edits = collect_edits(app);
    let dialog = gtk::Dialog::builder()
        .title("Pending changes")
        .transient_for(&app.window)
        .modal(true)
        .default_width(720)
        .default_height(480)
        .build();
    dialog.add_button("Close", gtk::ResponseType::Close);

    let view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .build();
    let buffer = view.buffer();
    if edits.is_empty() {
        buffer.set_text("No pending changes.");
    } else {
        let mut cache: HashMap<String, String> = HashMap::new();
        let mut diff = String::new();
        for pending in &edits {
            let text = cache
                .entry(pending.file.clone())
                .or_insert_with(|| std::fs::read_to_string(&pending.file).unwrap_or_default());
            let old = text.get(pending.start..pending.end).unwrap_or("");
            diff.push_str(&format!(
                "--- {}\n-{}\n+{}\n\n",
                pending.file, old, pending.value
            ));
        }
        buffer.set_text(&diff);
    }

    let scrolled = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(360)
        .build();
    dialog.content_area().append(&scrolled);
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

fn run_rebuild(app: &Rc<App>) {
    let config = app.config.borrow();
    let base = app.base.borrow().clone();
    let Some(command) = config.rebuild_command.clone() else {
        app.status.set_text("No rebuild command configured");
        return;
    };
    let terminal = config.terminal_command.clone();
    let mut process = match terminal {
        Some(prefix) if prefix.contains("{cmd}") => {
            let full = prefix.replace("{cmd}", &command);
            let mut parts = shell_words(&full);
            if parts.is_empty() {
                app.status.set_text("Empty terminal command");
                return;
            }
            let program = parts.remove(0);
            let mut cmd = std::process::Command::new(program);
            cmd.args(parts);
            cmd
        }
        Some(prefix) => {
            let parts = shell_words(&prefix);
            if parts.is_empty() {
                app.status.set_text("Empty terminal command");
                return;
            }
            let program = parts[0].clone();
            let mut cmd = std::process::Command::new(program);
            cmd.args(&parts[1..]);
            cmd.arg(&command);
            cmd
        }
        None => {
            let mut cmd = std::process::Command::new("sh");
            cmd.arg("-c").arg(&command);
            cmd
        }
    };
    process.current_dir(base);
    match process.spawn() {
        Ok(_) => app.status.set_text("Rebuild started in terminal"),
        Err(e) => app.status.set_text(&format!("Cannot start rebuild: {e}")),
    }
}

fn shell_words(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .map(|word| word.trim_matches('\'').trim_matches('"').to_string())
        .filter(|word| !word.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// git sync
// ---------------------------------------------------------------------------

/// Pull the flake directory, then report whether the tree is dirty or has
/// untracked files. Runs off the UI thread.
fn start_git_sync(app: &Rc<App>, tx: &Tx) {
    let dir = app.base.borrow().clone();
    if dir.is_empty() {
        app.status.set_text("No flake directory open");
        return;
    }
    app.status.set_text("Git: pulling...");
    spawn(tx, move |tx| {
        let result = run_git(&dir, &["pull"]).and_then(|_| {
            let status = run_git(&dir, &["status", "--porcelain"])?;
            Ok(!status.trim().is_empty())
        });
        let _ = tx.send_blocking(Msg::GitPulled(result));
    });
}

/// Stage every change in the flake directory, commit it, and push.
fn git_commit_and_push(dir: &str, message: &str) -> Result<String, String> {
    run_git(dir, &["add", "./*"])?;
    run_git(dir, &["commit", "-m", message])?;
    run_git(dir, &["push"])?;
    Ok(message.to_string())
}

/// Run a git subcommand in `dir`, returning stdout on success and a message on
/// failure.
fn run_git(dir: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(if detail.is_empty() {
        format!("git {} failed", args.join(" "))
    } else {
        format!("git {}: {detail}", args.join(" "))
    })
}

/// Ask for a commit message, then commit and push the staged changes.
fn commit_message_dialog(app: &Rc<App>, tx: &Tx) {
    let dialog = gtk::Dialog::builder()
        .title("Commit changes")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let commit = dialog.add_button("Commit & push", gtk::ResponseType::Accept);
    commit.add_css_class("suggested-action");

    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Commit message"));
    entry.set_hexpand(true);
    entry.set_activates_default(true);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.append(
        &gtk::Label::builder()
            .label("Commit message")
            .xalign(0.0)
            .build(),
    );
    content.append(&entry);
    dialog.content_area().append(&content);

    {
        let app = app.clone();
        let tx = tx.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let message = entry.text().to_string();
                let dir = app.base.borrow().clone();
                app.status.set_text("Git: committing and pushing...");
                spawn(&tx, move |tx| {
                    let result = git_commit_and_push(&dir, &message);
                    let _ = tx.send_blocking(Msg::GitSynced(result));
                });
            }
            dialog.close();
        });
    }
    dialog.present();
}

// ---------------------------------------------------------------------------
// nixd enrichment
// ---------------------------------------------------------------------------

/// Split an option path's segments into its parent path and its own name.
fn parent_segments(segments: &[String]) -> Option<(Vec<String>, String)> {
    let (name, parent) = segments.split_last()?;
    Some((parent.to_vec(), name.clone()))
}

fn query_current_page(app: &Rc<App>, tx: &Tx) {
    let file = app.current_file.borrow().clone();
    let target = app.target.borrow().clone();
    let base = app.base.borrow().clone();
    if target.is_empty() {
        set_nixd_status(app, "nixd: no option target for this config");
        return;
    }
    let paths: Vec<(String, Vec<String>)> = app
        .pages
        .borrow()
        .iter()
        .find(|page| page.file == file)
        .map(|page| {
            page.controls
                .iter()
                .map(|c| (c.record.path(), c.record.segments.clone()))
                .collect()
        })
        .unwrap_or_default();
    set_nixd_status(app, "Querying nixd...");
    spawn(tx, move |tx| {
        let parsed = Target::parse(&target);
        let mut parents: HashMap<String, Vec<metadata::ChildOption>> = HashMap::new();
        let mut infos = Vec::new();
        for (path, segments) in paths {
            if let Ok(info) = crate::nixd::query_option(&base, &target, &path) {
                if info.type_str.is_some() || !info.description.is_empty() {
                    infos.push((path, info));
                    continue;
                }
            }
            let (Some(target), Some((parent, name))) =
                (parsed.as_ref(), parent_segments(&segments))
            else {
                continue;
            };
            let key = parent.join(".");
            let children = parents
                .entry(key)
                .or_insert_with(|| metadata::enumerate(&base, target, &parent).unwrap_or_default());
            if let Some(child) = children.iter().find(|child| child.name == name) {
                infos.push((
                    path,
                    OptionInfo {
                        type_str: Some(child.type_str.clone()),
                        description: child.description.clone(),
                    },
                ));
            }
        }
        let _ = tx.send_blocking(Msg::Infos(file.clone(), infos));
    });
}

fn apply_infos(app: &Rc<App>, file: &str, infos: &[(String, OptionInfo)]) {
    let mut updated = 0;
    for page in app.pages.borrow().iter() {
        if page.file != file {
            continue;
        }
        for control in &page.controls {
            let Some((_, info)) = infos
                .iter()
                .find(|(path, _)| *path == control.record.path())
            else {
                continue;
            };
            if !info.description.is_empty() {
                control.desc.set_text(&info.description);
                control.desc.set_visible(true);
            }
            if let Some(type_str) = &info.type_str {
                if let Some(widget) = type_widget(type_str) {
                    let needs_rebuild = match &*control.editor.borrow() {
                        Editor::Raw { .. } => widget != WidgetKind::Raw,
                        _ => false,
                    };
                    if needs_rebuild && control.editor.borrow().unchanged(&control.record.value) {
                        let editor =
                            widgets::build(&control.record, Some(type_str), &build_context(app));
                        control.set_editor(editor);
                        connect_dirty(app, control);
                    }
                }
            }
            updated += 1;
        }
    }
    set_nixd_status(app, &format!("nixd updated {updated} option(s)"));
}

fn set_nixd_status(app: &Rc<App>, message: &str) {
    app.nixd_banner.set_title(message);
    app.nixd_banner.set_revealed(!message.is_empty());
}

fn type_widget(type_str: &str) -> Option<WidgetKind> {
    let lower = type_str.to_lowercase();
    if lower.contains("one of") {
        Some(WidgetKind::Dropdown)
    } else if lower.contains("bool") {
        Some(WidgetKind::Switch)
    } else if lower.contains("float") || lower.contains("number") || lower.contains("int") {
        Some(WidgetKind::Spin)
    } else if lower.contains("path") {
        Some(WidgetKind::Path)
    } else if lower.contains("package") {
        Some(WidgetKind::Package)
    } else if lower.contains("list") {
        Some(WidgetKind::List)
    } else if lower.contains("string") {
        Some(WidgetKind::Text)
    } else {
        Some(WidgetKind::Raw)
    }
}

// ---------------------------------------------------------------------------
// dialogs
// ---------------------------------------------------------------------------

fn open_configure(app: &Rc<App>) {
    let dialog = gtk::Dialog::builder()
        .title("Configure")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Save", gtk::ResponseType::Accept);

    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let config = app.config.borrow().clone();
    let flake_entry = gtk::Entry::new();
    flake_entry.set_text(config.flake_path.as_deref().unwrap_or(""));
    flake_entry.set_hexpand(true);
    let flake_browse = gtk::Button::with_label("Browse...");
    {
        let entry = flake_entry.clone();
        let window = app.window.clone();
        flake_browse.connect_clicked(move |_| select_folder(&window, &entry));
    }
    let flake_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    flake_row.append(&flake_entry);
    flake_row.append(&flake_browse);
    let terminal_entry = gtk::Entry::new();
    terminal_entry.set_text(config.terminal_command.as_deref().unwrap_or(""));
    terminal_entry.set_hexpand(true);
    let rebuild_entry = gtk::Entry::new();
    rebuild_entry.set_text(config.rebuild_command.as_deref().unwrap_or(""));
    rebuild_entry.set_hexpand(true);

    grid.attach(&gtk::Label::new(Some("Flake path")), 0, 0, 1, 1);
    grid.attach(&flake_row, 1, 0, 1, 1);
    grid.attach(&gtk::Label::new(Some("Terminal command")), 0, 1, 1, 1);
    grid.attach(&terminal_entry, 1, 1, 1, 1);
    grid.attach(&gtk::Label::new(Some("Rebuild command")), 0, 2, 1, 1);
    grid.attach(&rebuild_entry, 1, 2, 1, 1);

    dialog.content_area().append(&grid);

    {
        let app = app.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let flake = flake_entry.text().to_string();
                {
                    let mut config = app.config.borrow_mut();
                    config.terminal_command = non_empty(terminal_entry.text().to_string());
                    config.rebuild_command = non_empty(rebuild_entry.text().to_string());
                    if !flake.trim().is_empty() {
                        config.remember(flake.trim());
                    }
                }
                let _ = app.config.borrow().save();
                if !flake.trim().is_empty() {
                    let tx = app.tx.clone();
                    open_flake(&app, &tx, flake.trim());
                }
            }
            dialog.close();
        });
    }
    dialog.present();
}

fn non_empty(text: String) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Open a folder chooser and write the chosen path into `entry`.
fn select_folder(parent: &impl IsA<gtk::Window>, entry: &gtk::Entry) {
    let dialog = gtk::FileDialog::builder()
        .title("Select folder")
        .modal(true)
        .build();
    let entry = entry.clone();
    dialog.select_folder(
        Some(parent),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            if let Ok(folder) = result {
                if let Some(path) = folder.path() {
                    entry.set_text(&path.display().to_string());
                }
            }
        },
    );
}

fn open_add_file(app: &Rc<App>, tx: &Tx) {
    let root = app.base.borrow().clone();
    let templates = Rc::new(crate::templates::load_templates(&root));

    let dialog = gtk::Dialog::builder()
        .title("Add file")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Create", gtk::ResponseType::Accept);

    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let kind = gtk::ComboBoxText::new();
    kind.append(Some("raw"), "Raw function");
    kind.append(Some("nixos"), "NixOS module");
    kind.append(Some("home"), "Home-manager module");
    for (index, template) in templates.iter().enumerate() {
        kind.append(
            Some(&format!("template:{index}")),
            &format!("Template: {}", template.label),
        );
    }
    kind.set_active_id(Some("nixos"));

    let filename = gtk::Entry::new();
    filename.set_text("module.nix");
    let module_name = gtk::Entry::new();
    module_name.set_placeholder_text(Some("flake.modules.<kind>.<name>"));
    let directory = gtk::Entry::new();
    directory.set_text(&root);
    directory.set_hexpand(true);
    let directory_browse = gtk::Button::with_label("Browse...");
    {
        let entry = directory.clone();
        let window = app.window.clone();
        directory_browse.connect_clicked(move |_| select_folder(&window, &entry));
    }
    let directory_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    directory_row.append(&directory);
    directory_row.append(&directory_browse);

    grid.attach(&gtk::Label::new(Some("Kind")), 0, 0, 1, 1);
    grid.attach(&kind, 1, 0, 1, 1);
    grid.attach(&gtk::Label::new(Some("Filename")), 0, 1, 1, 1);
    grid.attach(&filename, 1, 1, 1, 1);
    grid.attach(&gtk::Label::new(Some("Module name")), 0, 2, 1, 1);
    grid.attach(&module_name, 1, 2, 1, 1);
    grid.attach(&gtk::Label::new(Some("Directory")), 0, 3, 1, 1);
    grid.attach(&directory_row, 1, 3, 1, 1);

    let placeholder_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    placeholder_box.set_margin_start(12);
    placeholder_box.set_margin_end(12);
    placeholder_box.set_margin_bottom(12);
    let placeholder_entries: Rc<RefCell<Vec<(String, gtk::Entry)>>> =
        Rc::new(RefCell::new(Vec::new()));

    {
        let filename = filename.clone();
        let directory = directory.clone();
        let placeholder_box = placeholder_box.clone();
        let placeholder_entries = placeholder_entries.clone();
        let root = root.clone();
        let templates = templates.clone();
        kind.connect_changed(move |combo| {
            while let Some(child) = placeholder_box.first_child() {
                placeholder_box.remove(&child);
            }
            placeholder_entries.borrow_mut().clear();
            let id = combo.active_id().map(|s| s.to_string()).unwrap_or_default();
            let Some(index) = id
                .strip_prefix("template:")
                .and_then(|s| s.parse::<usize>().ok())
            else {
                filename.set_text("module.nix");
                directory.set_text(&root);
                return;
            };
            let Some(template) = templates.get(index) else {
                return;
            };
            filename.set_text(&template.filename);
            directory.set_text(&Path::new(&root).join(&template.output).to_string_lossy());
            for placeholder in &template.placeholders {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let label = gtk::Label::builder()
                    .label(&placeholder.label)
                    .xalign(0.0)
                    .build();
                let entry = gtk::Entry::new();
                entry.set_text(&placeholder.default);
                entry.set_hexpand(true);
                if !placeholder.hint.is_empty() {
                    entry.set_placeholder_text(Some(&placeholder.hint));
                }
                row.append(&label);
                row.append(&entry);
                placeholder_box.append(&row);
                placeholder_entries
                    .borrow_mut()
                    .push((placeholder.name.clone(), entry));
            }
        });
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&grid);
    content.append(&placeholder_box);
    dialog.content_area().append(&content);

    {
        let app = app.clone();
        let tx = tx.clone();
        let templates = templates.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let kind_id = kind
                    .active_id()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "nixos".into());
                let module_name_text = module_name.text().to_string();
                let path = Path::new(&directory.text()).join(filename.text());
                let result = match kind_id
                    .strip_prefix("template:")
                    .and_then(|s| s.parse::<usize>().ok())
                    .and_then(|index| templates.get(index))
                {
                    Some(template) => {
                        let values: Vec<(String, String)> = placeholder_entries
                            .borrow()
                            .iter()
                            .map(|(name, entry)| (name.clone(), entry.text().to_string()))
                            .collect();
                        let body = crate::templates::render(&template.body, &values);
                        write_body(&path, &body)
                    }
                    None => write_template(&kind_id, &module_name_text, &path),
                };
                match result {
                    Ok(()) => {
                        app.status.set_text(&format!("Created {}", path.display()));
                        let base = app.base.borrow().clone();
                        let path_clone = base.clone();
                        spawn(&tx, move |tx| {
                            let result = scan::scan_root(&path_clone);
                            let _ = tx.send_blocking(Msg::Scanned(result));
                        });
                        if !Path::new(&base).join("flake.nix").is_file() {
                            app.status.set_text("Created; add it to the flake imports");
                        }
                    }
                    Err(e) => app.status.set_text(&format!("Cannot create file: {e}")),
                }
            }
            dialog.close();
        });
    }
    dialog.present();
}

fn write_body(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, body).map_err(|e| e.to_string())
}

fn write_template(kind: &str, module_name: &str, path: &Path) -> Result<(), String> {
    let displayname = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "New menu".into());
    let template = template_for(&displayname, kind, module_name, path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, template).map_err(|e| e.to_string())
}

/// Build a new module file. Raw files are plain functions for non-flake-parts
/// use; NixOS and home-manager files wrap their body in the flake-parts
/// `flake.modules.<kind>.<name>` container. The module name falls back to the
/// file stem when the user leaves it blank.
fn template_for(displayname: &str, kind: &str, module_name: &str, path: &Path) -> String {
    let header = format!("/* @menu\n   displayname = \"{displayname}\";\n   order = 1000;\n*/\n\n");
    let name = if module_name.trim().is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "module".into())
    } else {
        module_name.trim().to_string()
    };
    let body = match kind {
        "raw" => "{ ... }:\n\n{\n}\n".to_string(),
        "home" => format!(
            "{{ self, inputs, ... }}:\n\n{{\n\nflake.modules.homeManager.{name} = {{ pkgs, ... }}: {{\n\n}};\n}}\n"
        ),
        _ => format!(
            "{{ self, inputs, ... }}:\n\n{{\n\nflake.modules.nixos.{name} = {{ pkgs, ... }}: {{\n\n}};\n}}\n"
        ),
    };
    format!("{header}{body}")
}

/// Prompt for a snippet's `%%token%%` values, render it, and insert it at the
/// marker's line so the marker shifts down one line.
fn open_snippet_dialog(app: &Rc<App>, file: &str, marker: &InsertMarker, snippet: &SnippetSpec) {
    let title_label = strip_add_prefix(&snippet.label);
    let title = if title_label.is_empty() {
        "Add".to_string()
    } else {
        format!("Add {title_label}")
    };
    let dialog = gtk::Dialog::builder()
        .title(title)
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Add", gtk::ResponseType::Accept);

    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let mut entries: Vec<(String, gtk::Entry)> = Vec::new();
    for (row, placeholder) in snippet.placeholders.iter().enumerate() {
        let label = gtk::Label::builder()
            .label(&placeholder.label)
            .xalign(0.0)
            .build();
        let entry = gtk::Entry::new();
        entry.set_text(&placeholder.default);
        entry.set_hexpand(true);
        entry.set_width_chars(40);
        if !placeholder.hint.is_empty() {
            entry.set_placeholder_text(Some(&placeholder.hint));
        }
        grid.attach(&label, 0, row as i32, 1, 1);
        grid.attach(&entry, 1, row as i32, 1, 1);
        entries.push((placeholder.name.clone(), entry));
    }
    dialog.content_area().append(&grid);

    let file = file.to_string();
    let offset = marker.offset;
    let body = snippet.body.clone();
    let tx = app.tx.clone();
    let master: gtk::Window = app.window.clone().upcast();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            let values: Vec<(String, String)> = entries
                .iter()
                .map(|(name, entry)| (name.clone(), entry.text().to_string()))
                .collect();
            let rendered = crate::templates::render(&body, &values);
            let result = edit::insert_at(&file, offset, &rendered);
            let message = match &result {
                Ok(()) => Msg::SnippetInserted(Ok(())),
                Err(e) => {
                    show_error(&master, e);
                    Msg::SnippetInserted(Err(e.clone()))
                }
            };
            let _ = tx.send_blocking(message);
        }
        dialog.close();
    });
    dialog.present();
}

fn open_add_option(app: &Rc<App>, file: &str, origin: &str) {
    let dialog = gtk::Dialog::builder()
        .title("Add option")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Add", gtk::ResponseType::Accept);

    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let path_entry = gtk::Entry::new();
    path_entry.set_placeholder_text(Some("services.pipewire.enable"));
    path_entry.set_hexpand(true);
    path_entry.set_width_chars(48);
    let completion = gtk::EntryCompletion::new();
    let store = gtk::ListStore::new(&[glib::Type::STRING]);
    completion.set_model(Some(&store));
    completion.set_text_column(0);
    completion.set_minimum_key_length(2);
    path_entry.set_completion(Some(&completion));
    for path in option_suggestions(app) {
        store.set(&store.append(), &[(0, &path)]);
    }
    request_suggestions(app, store.clone(), Vec::new());
    {
        let app = app.clone();
        let store = store.clone();
        let generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
        path_entry.connect_changed(move |entry| {
            let current = generation.get() + 1;
            generation.set(current);
            let parent = parent_segments_from_text(&entry.text());
            let app = app.clone();
            let store = store.clone();
            let generation = generation.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                if generation.get() != current {
                    return;
                }
                request_suggestions(&app, store, parent)
            });
        });
    }

    let widget = gtk::ComboBoxText::new();
    for kind in [
        "switch",
        "dropdown",
        "radio",
        "spin",
        "slider",
        "text",
        "multiline",
        "path",
        "package",
        "packages",
        "list",
        "color",
        "submodule",
        "raw",
    ] {
        widget.append(Some(kind), kind);
    }
    widget.set_active_id(Some("text"));

    let label_entry = gtk::Entry::new();
    label_entry.set_placeholder_text(Some("Optional label"));

    let value_entry = gtk::Entry::new();
    value_entry.set_placeholder_text(Some("Initial value"));
    value_entry.set_width_chars(48);

    grid.attach(&gtk::Label::new(Some("Option path")), 0, 0, 1, 1);
    grid.attach(&path_entry, 1, 0, 1, 1);
    grid.attach(&gtk::Label::new(Some("Widget")), 0, 1, 1, 1);
    grid.attach(&widget, 1, 1, 1, 1);
    grid.attach(&gtk::Label::new(Some("Label")), 0, 2, 1, 1);
    grid.attach(&label_entry, 1, 2, 1, 1);
    grid.attach(&gtk::Label::new(Some("Value")), 0, 3, 1, 1);
    grid.attach(&value_entry, 1, 3, 1, 1);

    dialog.content_area().append(&grid);

    let file = file.to_string();
    let origin = origin.to_string();
    let tx = app.tx.clone();
    let master: gtk::Window = app.window.clone().upcast();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            let option_path = path_entry.text().trim().to_string();
            let widget_id = widget.active_id().unwrap_or_else(|| "text".into());
            let label = label_entry.text().to_string();
            let value_text = value_entry.text().to_string();
            if !option_path.is_empty() {
                let block = build_assignment_block(&option_path, &widget_id, &label, &value_text);
                let result = edit::insert_assignment(&file, &origin, &block);
                let message = match &result {
                    Ok(()) => Msg::Inserted(Ok(())),
                    Err(e) => {
                        show_error(&master, e);
                        Msg::Inserted(Err(e.clone()))
                    }
                };
                let _ = tx.send_blocking(message);
            }
        }
        dialog.close();
    });
    dialog.present();
}

/// Every option path the scan already knows, plus each dotted prefix, offered
/// as autocomplete for the add-option dialog.
fn option_suggestions(app: &Rc<App>) -> Vec<String> {
    let scan = app.scan.borrow();
    let Some(scan) = scan.as_ref() else {
        return Vec::new();
    };
    let mut set = std::collections::BTreeSet::new();
    for menu in &scan.menus {
        for record in &menu.records {
            let segments = &record.segments;
            for depth in 1..=segments.len() {
                set.insert(segments[..depth].join("."));
            }
        }
    }
    set.into_iter().collect()
}

/// Ask a worker to enumerate the children of `parent` and offer those paths to
/// the add-option entry's completion store, reusing a cached result when present.
fn request_suggestions(app: &Rc<App>, store: gtk::ListStore, parent: Vec<String>) {
    *app.suggest_store.borrow_mut() = Some(store);
    let key = parent.join(".");
    *app.suggest_parent.borrow_mut() = Some(key.clone());
    let cached = app.suggest_cache.borrow().get(&key).cloned();
    if let Some(cached) = cached {
        fill_suggestions(app, cached);
        return;
    }
    let base = app.base.borrow().clone();
    let target = app.target.borrow().clone();
    let tx = app.tx.clone();
    spawn(&tx, move |tx| {
        let Some(target) = Target::parse(&target) else {
            let _ = tx.send_blocking(Msg::Suggestions(Vec::new()));
            return;
        };
        let mut paths: Vec<String> = metadata::enumerate(&base, &target, &parent)
            .unwrap_or_default()
            .into_iter()
            .map(|child| child.path)
            .collect();
        paths.sort();
        paths.dedup();
        let _ = tx.send_blocking(Msg::Suggestions(paths));
    });
}

/// The parent path implied by the text typed so far: everything before the last
/// dot, split into segments. `services.pip` yields `["services"]`; a bare
/// `services` yields the root.
fn parent_segments_from_text(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    match trimmed.rfind('.') {
        Some(index) => trimmed[..index]
            .split('.')
            .filter(|segment| !segment.is_empty())
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    }
}

/// Merge the fetched children into the completion store, remembering them under
/// the parent key so revisiting a prefix is instant.
fn fill_suggestions(app: &Rc<App>, suggestions: Vec<String>) {
    let Some(store) = app.suggest_store.borrow_mut().take() else {
        return;
    };
    if let Some(key) = app.suggest_parent.borrow_mut().take() {
        app.suggest_cache
            .borrow_mut()
            .insert(key, suggestions.clone());
    }
    let mut paths: std::collections::BTreeSet<String> =
        option_suggestions(app).into_iter().collect();
    paths.extend(suggestions);
    store.clear();
    for path in paths {
        store.set(&store.append(), &[(0, &path)]);
    }
}

fn build_assignment_block(option_path: &str, widget: &str, label: &str, value: &str) -> String {
    let kind = WidgetKind::parse(widget).unwrap_or(WidgetKind::Raw);
    let mut comment = String::from("/* @option\n");
    comment.push_str(&format!("   widget = \"{widget}\";\n"));
    if !label.trim().is_empty() {
        comment.push_str(&format!("   label = \"{}\";\n", label.trim()));
    }
    comment.push_str("*/\n");
    let literal = widgets::literal_for_widget(kind, value);
    format!("{comment}{option_path} = {literal};")
}

// ---------------------------------------------------------------------------
// message pump
// ---------------------------------------------------------------------------

fn watch_files(app: &Rc<App>) {
    app.watchers.borrow_mut().clear();
    let mut files: Vec<String> = Vec::new();
    {
        let scan = app.scan.borrow();
        let Some(scan) = scan.as_ref() else {
            return;
        };
        for menu in &scan.menus {
            for record in &menu.records {
                if !record.file.is_empty() && !files.contains(&record.file) {
                    files.push(record.file.clone());
                }
            }
        }
    }
    for file in files {
        let Ok(monitor) = gio::File::for_path(&file)
            .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        else {
            continue;
        };
        let handler_app = app.clone();
        monitor.connect_changed(move |_, _, _, _| {
            if collect_edits(&handler_app).is_empty() {
                let base = handler_app.base.borrow().clone();
                spawn(&handler_app.tx, move |tx| {
                    let result = scan::scan_root(&base);
                    let _ = tx.send_blocking(Msg::Scanned(result));
                });
            } else {
                handler_app
                    .status
                    .set_text("File changed on disk; reload to refresh");
            }
        });
        app.watchers.borrow_mut().push(monitor);
    }
}

fn handle_msg(app: &Rc<App>, tx: &Tx, msg: Msg) {
    match msg {
        Msg::Scanned(Ok(result)) => {
            *app.targets.borrow_mut() = result.targets.clone();
            let hosts = scan::hosts_from_targets(&result.targets);
            if !app.manual_host.get() {
                let hostname = glib::host_name().to_string();
                if let Some(host) = hosts
                    .iter()
                    .find(|h| **h == hostname || hostname.starts_with(&format!("{h}.")))
                {
                    *app.current_host.borrow_mut() = host.clone();
                    app.system_toggle.set_active(true);
                    app.system_only.set(true);
                }
            }
            let dendritic = result
                .menus
                .iter()
                .any(|menu| menu.records.iter().any(|r| !r.origin.is_empty()));
            *app.hosts.borrow_mut() = hosts.clone();
            app.host_combo.remove_all();
            for host in &hosts {
                app.host_combo.append_text(host);
            }
            if let Some(first) = hosts.first() {
                let current = app.current_host.borrow().clone();
                if current.is_empty() {
                    *app.current_host.borrow_mut() = first.clone();
                }
                let selected = app
                    .host_combo
                    .active_text()
                    .map(|t| t.to_string())
                    .filter(|t| hosts.contains(t))
                    .unwrap_or_else(|| app.current_host.borrow().clone());
                let index = hosts.iter().position(|h| *h == selected).unwrap_or(0);
                app.host_combo.set_active(Some(index as u32));
                *app.current_host.borrow_mut() = hosts[index].clone();
            }
            let target = if dendritic {
                "fp".to_string()
            } else if let Some(host) = hosts.first() {
                format!("nixos:{host}")
            } else {
                String::new()
            };
            *app.target.borrow_mut() = target;

            let path = app.base.borrow().clone();
            let name = Path::new(&path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or(path);
            app.title.set_text(&format!("nix-settings-gui - {name}"));
            *app.scan.borrow_mut() = Some(result);
            watch_files(app);
            app.status.set_text("Ready");
            rebuild_view(app);
        }
        Msg::Scanned(Err(e)) => {
            app.status.set_text("Scan failed");
            show_error(&app.window, &e);
        }
        Msg::SaveProgress(fraction, label) => {
            set_busy(app, &label, fraction);
        }
        Msg::Saved(Ok(count)) => {
            clear_busy(app);
            app.status.set_text(&format!("Saved {count} file(s)"));
            let pending = app.pending_rebuild.replace(false);
            let base = app.base.borrow().clone();
            spawn(tx, move |tx| {
                let result = scan::scan_root(&base);
                let _ = tx.send_blocking(Msg::Scanned(result));
            });
            if pending {
                run_rebuild(app);
            }
        }
        Msg::Saved(Err(e)) => {
            clear_busy(app);
            app.pending_rebuild.set(false);
            app.status.set_text("Save failed");
            show_error(&app.window, &e);
        }
        Msg::Invalid(errors) => {
            clear_busy(app);
            app.pending_rebuild.set(false);
            app.status
                .set_text(&format!("Package error: {}", errors.join("; ")));
        }
        Msg::Inserted(Ok(())) => {
            app.status.set_text("Option added");
            let base = app.base.borrow().clone();
            spawn(tx, move |tx| {
                let result = scan::scan_root(&base);
                let _ = tx.send_blocking(Msg::Scanned(result));
            });
        }
        Msg::Inserted(Err(e)) => {
            app.status.set_text("Add option failed");
            show_error(&app.window, &e);
        }
        Msg::SnippetInserted(Ok(())) => {
            app.status.set_text("Snippet added");
            let base = app.base.borrow().clone();
            spawn(tx, move |tx| {
                let result = scan::scan_root(&base);
                let _ = tx.send_blocking(Msg::Scanned(result));
            });
        }
        Msg::SnippetInserted(Err(e)) => {
            app.status.set_text("Add snippet failed");
            show_error(&app.window, &e);
        }
        Msg::Infos(file, infos) => apply_infos(app, &file, &infos),
        Msg::Suggestions(suggestions) => fill_suggestions(app, suggestions),
        Msg::GitPulled(Ok(true)) => commit_message_dialog(app, tx),
        Msg::GitPulled(Ok(false)) => app.status.set_text("Git: already up to date"),
        Msg::GitPulled(Err(e)) => {
            app.status.set_text("Git sync failed");
            show_error(&app.window, &e);
        }
        Msg::GitSynced(Ok(_)) => app.status.set_text("Git: synced"),
        Msg::GitSynced(Err(e)) => {
            app.status.set_text("Git sync failed");
            show_error(&app.window, &e);
        }
    }
}

fn show_error(parent: &impl IsA<gtk::Window>, message: &str) {
    let dialog = gtk::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .message_type(gtk::MessageType::Error)
        .buttons(gtk::ButtonsType::Close)
        .text(message)
        .build();
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

fn spawn<F: FnOnce(Tx) + Send + 'static>(tx: &Tx, work: F) {
    let tx = tx.clone();
    std::thread::spawn(move || work(tx));
}

fn discover_flakes(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 4 {
            return;
        }
        if dir.join("flake.nix").is_file() {
            out.push(dir.to_path_buf());
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(
                    name.as_ref(),
                    "target" | "result" | "node_modules" | ".git" | ".direnv"
                )
            {
                continue;
            }
            walk(&path, depth + 1, out);
        }
    }
    if root.is_dir() {
        walk(root, 0, &mut found);
    }
    found.sort();
    found.dedup();
    found
}

fn populate_flakes(app: &Rc<App>) -> Option<String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut flakes = discover_flakes(&home.join("repos"));
    let config = app.config.borrow().clone();
    let mut recents: Vec<PathBuf> = Vec::new();
    for path in config
        .recent_flake_paths
        .iter()
        .chain(config.flake_path.iter())
    {
        let path = PathBuf::from(path);
        if !recents.contains(&path) && !flakes.contains(&path) {
            recents.push(path);
        }
    }
    for path in recents.into_iter().rev() {
        flakes.insert(0, path);
    }
    app.flake_combo.remove_all();
    for flake in &flakes {
        let label = flake
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| flake.display().to_string());
        app.flake_combo
            .append(Some(&flake.display().to_string()), &label);
    }
    *app.flakes.borrow_mut() = flakes;
    if app.flakes.borrow().is_empty() {
        return None;
    }
    let index = app
        .config
        .borrow()
        .flake_path
        .as_deref()
        .and_then(|path| {
            app.flakes
                .borrow()
                .iter()
                .position(|f| f.display().to_string() == path)
        })
        .or_else(|| {
            app.flakes
                .borrow()
                .iter()
                .position(|f| f.display().to_string().ends_with("dendrite"))
        })
        .unwrap_or(0);
    let path = app.flakes.borrow()[index].display().to_string();
    app.flake_combo.set_active(Some(index as u32));
    Some(path)
}

pub fn run(app: &adw::Application) -> glib::ExitCode {
    crate::nixd::init();
    let (tx, rx) = async_channel::unbounded::<Msg>();
    let rx_cell = std::cell::Cell::new(Some(rx));

    app.connect_activate(move |app| {
        let ui = build_ui(app, &tx);
        if let Some(rx) = rx_cell.take() {
            let ui_for_rx = ui.clone();
            let tx_for_rx = tx.clone();
            glib::MainContext::default().spawn_local(async move {
                while let Ok(msg) = rx.recv().await {
                    handle_msg(&ui_for_rx, &tx_for_rx, msg);
                }
            });
        }
        if let Some(path) = populate_flakes(&ui) {
            open_flake(&ui, &tx, &path);
        } else {
            ui.status
                .set_text("No flake found; use Configure to set a path");
        }
        ui.window.present();
    });

    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_prompt_responses() {
        assert_eq!(changes_action(gtk::ResponseType::Yes), ChangesAction::Save);
        assert_eq!(
            changes_action(gtk::ResponseType::No),
            ChangesAction::Discard
        );
        assert_eq!(
            changes_action(gtk::ResponseType::Cancel),
            ChangesAction::Cancel
        );
        assert_eq!(
            changes_action(gtk::ResponseType::DeleteEvent),
            ChangesAction::Cancel
        );
    }

    #[test]
    fn splits_option_paths() {
        let segments = vec!["services".to_string(), "openssh".to_string()];
        assert_eq!(
            parent_segments(&segments),
            Some((vec!["services".to_string()], "openssh".to_string()))
        );
        assert_eq!(
            parent_segments(&segments[..1]),
            Some((Vec::new(), "services".to_string()))
        );
        assert_eq!(parent_segments(&[]), None);
    }

    #[test]
    fn offsets_scroll_above_the_target() {
        assert_eq!(scroll_offset(120.0), 108.0);
        assert_eq!(scroll_offset(0.0), -12.0);
    }

    #[test]
    fn hides_group_when_all_rows_invisible() {
        let visible = visible_group_names([
            ((false, "Firefox".to_string()), false),
            ((false, "Firefox".to_string()), false),
        ]);
        assert!(!visible.contains(&(false, "Firefox".to_string())));
    }

    #[test]
    fn shows_group_when_one_row_visible() {
        let visible = visible_group_names([
            ((false, "Chrome".to_string()), false),
            ((false, "Chrome".to_string()), true),
        ]);
        assert!(visible.contains(&(false, "Chrome".to_string())));
    }

    #[test]
    fn separates_same_group_name_by_section() {
        let visible = visible_group_names([
            ((false, "Theme".to_string()), false),
            ((true, "Theme".to_string()), true),
        ]);
        assert!(!visible.contains(&(false, "Theme".to_string())));
        assert!(visible.contains(&(true, "Theme".to_string())));
    }

    #[test]
    fn empty_rows_yield_empty_visible_set() {
        let visible = visible_group_names(Vec::<((bool, String), bool)>::new());
        assert!(visible.is_empty());
    }

    #[test]
    fn writes_kind_specific_templates() {
        let path = Path::new("/tmp/leaf.nix");

        let raw = template_for("leaf", "raw", "", path);
        assert!(raw.contains("{ ... }:"));
        assert!(raw.ends_with("{\n}\n"));
        assert!(!raw.contains("flake.modules"));

        let nixos = template_for("leaf", "nixos", "audio", path);
        assert!(nixos.contains("{ self, inputs, ... }:"));
        assert!(nixos.contains("flake.modules.nixos.audio = { pkgs, ... }:"));

        let home = template_for("leaf", "home", "shell", path);
        assert!(home.contains("flake.modules.homeManager.shell = { pkgs, ... }:"));

        let fallback = template_for("leaf", "nixos", "   ", path);
        assert!(fallback.contains("flake.modules.nixos.leaf = { pkgs, ... }:"));
    }

    #[test]
    fn derives_suggestion_parent_from_text() {
        assert!(parent_segments_from_text("services").is_empty());
        assert_eq!(
            parent_segments_from_text("services.pip"),
            vec!["services".to_string()]
        );
        assert_eq!(
            parent_segments_from_text("environment.systemPackages"),
            vec!["environment".to_string()]
        );
        assert_eq!(
            parent_segments_from_text("  services.pipewire."),
            vec!["services".to_string(), "pipewire".to_string()]
        );
    }

    #[test]
    fn resolves_dependency_paths_across_config_wrapper() {
        let mut values = HashMap::new();
        values.insert(
            "config.ojuny.browser.selected".to_string(),
            "\"firefox\"".to_string(),
        );
        assert_eq!(
            resolve_dependency_value(&values, "ojuny.browser.selected").map(String::as_str),
            Some("\"firefox\"")
        );
        values.insert("programs.steam.enable".to_string(), "false".to_string());
        assert_eq!(
            resolve_dependency_value(&values, "programs.steam.enable").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            resolve_dependency_value(&values, "config.programs.steam.enable").map(String::as_str),
            Some("false")
        );
        assert_eq!(resolve_dependency_value(&values, "missing.path"), None);
    }

    #[test]
    fn visible_when_resolves_config_wrapped_scan_paths() {
        let text = "{ config = {\n  # @option widget = \"switch\"; label = \"Set gaps per edge\";\n  ojuny.style.customGaps = false;\n  # @option widget = \"spin\"; label = \"Gap size\"; visibleWhen.path = \"ojuny.style.customGaps\"; visibleWhen.equals = false;\n  ojuny.style.gaps = 4;\n}; }";
        let records = scan::scan_value(text);
        let mut values: HashMap<String, String> = records
            .iter()
            .map(|record| (record.path(), record.value.clone()))
            .collect();
        let gated = records
            .iter()
            .find(|record| record.spec.visible_when.is_some())
            .unwrap();
        let vis = gated.spec.visible_when.as_ref().unwrap();
        assert_eq!(vis.path, "ojuny.style.customGaps");
        assert!(values.contains_key("config.ojuny.style.customGaps"));
        assert!(dependency_visible(vis, &values));
        values.insert("config.ojuny.style.customGaps".to_string(), "true".to_string());
        assert!(!dependency_visible(vis, &values));
    }

    #[test]
    fn missing_menu_icon_falls_back_to_generic() {
        let only_fallback = |name: &str| name == "preferences-system-symbolic";
        assert_eq!(
            pick_icon_name(only_fallback, Some("preferences-system-displays-symbolic")).as_deref(),
            Some("preferences-system-symbolic")
        );
        let all_present = |_: &str| true;
        assert_eq!(
            pick_icon_name(all_present, Some("web-browser-symbolic")).as_deref(),
            Some("web-browser-symbolic")
        );
        assert_eq!(pick_icon_name(only_fallback, None), None);
    }
}
