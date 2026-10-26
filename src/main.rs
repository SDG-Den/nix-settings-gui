#![allow(deprecated)]

use gtk::glib;

mod app;
mod apply;
mod config;
mod dsl;
mod edit;
#[allow(dead_code)]
mod metadata;
#[allow(dead_code)]
mod model;
mod monitor_detect;
mod nixd;
mod plugin_list;
mod plugin_schema;
mod scan;
#[allow(dead_code)]
mod templates;
#[allow(dead_code)]
mod value;
mod widgets;

fn main() -> glib::ExitCode {
    glib::set_application_name("nix-settings-gui");
    gtk::Window::set_default_icon_name("preferences-system");
    let application = adw::Application::builder()
        .application_id(app::APP_ID)
        .build();
    app::run(&application)
}
