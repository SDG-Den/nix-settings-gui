use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use regex::Regex;

use crate::config::AppConfig;

mod apply;
mod controls;
mod dialogs;
mod enrich;
mod files;
mod git;
mod icon;
mod inputs;
pub(crate) mod menu;
mod pump;
mod save;
mod state;
mod wallpaper;

use apply::ApplyState;
use controls::apply_control_filter;
use dialogs::{open_add_file, open_configure};
use enrich::query_current_page;
use git::start_git_sync;
use menu::{open_flake, rebuild_menu_list, rebuild_view};
use pump::{handle_msg, populate_flakes};
use save::{collect_edits, discard_changes, preview_changes, reload, run_changes_prompt, save_all};
use state::{App, Msg, PackageService, PluginRegistryState, Tx};

pub const APP_ID: &str = "org.nix_settings_gui.NixSettingsGui";

// ---------------------------------------------------------------------------
// UI construction
// ---------------------------------------------------------------------------

fn preference_row(label: &str, tooltip: &str, switch: &gtk::Switch) -> gtk::Box {
    let text = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .tooltip_text(tooltip)
        .build();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&text);
    row.append(switch);
    row
}

fn build_ui(app: &adw::Application, tx: &Tx, page_pattern: Option<Regex>) -> Rc<App> {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        ".search-hit { outline: 2px solid @accent_color; border-radius: 6px; }\n\
         .group-header { color: @accent_color; font-weight: 700; }\n\
         .busy-scrim { background-color: rgba(0, 0, 0, 0.45); }\n\
         .busy-spinner { color: white; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let title = gtk::Label::builder()
        .label("settings")
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

    let inputs_list = gtk::ListBox::new();
    inputs_list.set_selection_mode(gtk::SelectionMode::Single);
    inputs_list.add_css_class("navigation-sidebar");
    let inputs_row = gtk::ListBoxRow::new();
    inputs_row.set_widget_name(inputs::INPUTS_PAGE);
    let inputs_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    inputs_content.set_margin_top(6);
    inputs_content.set_margin_bottom(6);
    inputs_content.set_margin_start(6);
    inputs_content.set_margin_end(6);
    if let Some(icon) = inputs::flake_icon_image() {
        inputs_content.append(&icon);
    }
    inputs_content.append(
        &gtk::Label::builder()
            .label("Flake Inputs")
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    let inputs_warning = gtk::Image::from_icon_name("dialog-warning-symbolic");
    inputs_warning.set_visible(false);
    inputs_content.append(&inputs_warning);
    inputs_row.set_child(Some(&inputs_content));
    inputs_list.append(&inputs_row);

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.append(&list_scroll);
    sidebar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    sidebar.append(&inputs_list);

    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    let inputs_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    stack.add_named(&inputs_page, Some(inputs::INPUTS_PAGE));

    let nixd_banner = adw::Banner::new("");
    nixd_banner.set_revealed(false);

    let field_spinner = gtk::Spinner::new();
    field_spinner.set_halign(gtk::Align::Center);
    field_spinner.set_valign(gtk::Align::Center);
    field_spinner.set_hexpand(true);
    field_spinner.set_vexpand(true);
    field_spinner.set_visible(true);
    field_spinner.add_css_class("busy-spinner");

    let field_scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
    field_scrim.set_halign(gtk::Align::Fill);
    field_scrim.set_valign(gtk::Align::Fill);
    field_scrim.set_hexpand(true);
    field_scrim.set_vexpand(true);
    field_scrim.set_visible(false);
    field_scrim.add_css_class("busy-scrim");
    field_scrim.append(&field_spinner);
    {
        let spinner = field_spinner.clone();
        field_scrim.connect_map(move |_| spinner.start());
    }
    {
        let spinner = field_spinner.clone();
        field_scrim.connect_unmap(move |_| spinner.stop());
    }

    let content_overlay = gtk::Overlay::new();
    content_overlay.set_child(Some(&stack));
    content_overlay.add_overlay(&field_scrim);

    let content_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_pane.append(&content_overlay);
    content_pane.append(&nixd_banner);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&sidebar));
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

    let apply_spinner = gtk::Spinner::new();
    apply_spinner.set_visible(false);
    apply_spinner.set_tooltip_text(Some("Applying the changed setting..."));

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

    let live_apply_switch = gtk::Switch::new();
    menu_box.append(&preference_row(
        "Apply changes immediately",
        "Run each option's applyCommand as soon as it changes",
        &live_apply_switch,
    ));
    let expand_details_switch = gtk::Switch::new();
    menu_box.append(&preference_row(
        "Expand setting details",
        "Show setting descriptions and import origins by default",
        &expand_details_switch,
    ));
    let mango_reload_switch = gtk::Switch::new();
    menu_box.append(&preference_row(
        "Reload mango after rebuild",
        "Run mmsg dispatch reload_config after a successful rebuild",
        &mango_reload_switch,
    ));
    let dms_reload_switch = gtk::Switch::new();
    menu_box.append(&preference_row(
        "Reload DMS after rebuild",
        "Run dms kill && mmsg dispatch spawn_shell,dms run after a successful rebuild",
        &dms_reload_switch,
    ));

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
    bottom.append(&apply_spinner);
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
        .title("settings")
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
        inputs_list,
        inputs_page,
        inputs_warning,
        showing_inputs: Cell::new(false),
        stack,
        nixd_banner,
        status,
        spinner,
        apply_spinner,
        progress,
        field_scrim,
        saving: Cell::new(false),
        scans_in_flight: Cell::new(0),
        package_service: PackageService::new(),
        plugin_registry: Rc::new(PluginRegistryState::new()),
        wallpaper_set: RefCell::new(None),
        apply_state: ApplyState::new(),
        tx: tx.clone(),
        config: RefCell::new(AppConfig::load()),
        base: RefCell::new(String::new()),
        scan: RefCell::new(None),
        targets: RefCell::new(Vec::new()),
        target: RefCell::new(String::new()),
        hosts: RefCell::new(Vec::new()),
        current_host: RefCell::new(String::new()),
        current_file: RefCell::new(String::new()),
        page_pattern,
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

    {
        let config = app_state.config.borrow();
        live_apply_switch.set_active(config.live_apply);
        expand_details_switch.set_active(config.expand_details_by_default);
        mango_reload_switch.set_active(config.reload_mango_on_rebuild);
        dms_reload_switch.set_active(config.reload_dms_on_rebuild);
    }

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
    wire_preferences(
        &app_state,
        &live_apply_switch,
        &expand_details_switch,
        &mango_reload_switch,
        &dms_reload_switch,
    );
    inputs::refresh(&app_state);
    {
        let app = app_state.clone();
        app_state
            .inputs_list
            .clone()
            .connect_row_selected(move |_, row| {
                if row.is_none() {
                    return;
                }
                app.showing_inputs.set(true);
                app.list.unselect_all();
                app.stack.set_visible_child_name(inputs::INPUTS_PAGE);
                inputs::refresh(&app);
            });
    }
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
            app.showing_inputs.set(false);
            app.inputs_list.unselect_all();
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

fn wire_preferences(
    app: &Rc<App>,
    live_apply: &gtk::Switch,
    expand_details: &gtk::Switch,
    reload_mango: &gtk::Switch,
    reload_dms: &gtk::Switch,
) {
    {
        let app = app.clone();
        live_apply.connect_active_notify(move |switch| {
            app.config.borrow_mut().live_apply = switch.is_active();
            let _ = app.config.borrow().save();
            controls::refresh_live_markers(&app);
        });
    }
    {
        let app = app.clone();
        expand_details.connect_active_notify(move |switch| {
            let expanded = switch.is_active();
            app.config.borrow_mut().expand_details_by_default = expanded;
            let _ = app.config.borrow().save();
            controls::set_all_details_expanded(&app, expanded);
        });
    }
    {
        let app = app.clone();
        reload_mango.connect_active_notify(move |switch| {
            app.config.borrow_mut().reload_mango_on_rebuild = switch.is_active();
            let _ = app.config.borrow().save();
        });
    }
    {
        let app = app.clone();
        reload_dms.connect_active_notify(move |switch| {
            app.config.borrow_mut().reload_dms_on_rebuild = switch.is_active();
            let _ = app.config.borrow().save();
        });
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

pub fn run(app: &adw::Application, page_pattern: Option<Regex>) -> glib::ExitCode {
    crate::nixd::init();
    let (tx, rx) = async_channel::unbounded::<Msg>();
    let rx_cell = std::cell::Cell::new(Some(rx));

    app.connect_activate(move |app| {
        let ui = build_ui(app, &tx, page_pattern.clone());
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

    app.run_with_args::<&str>(&[])
}
