#![allow(deprecated)]

use gtk::glib;
use gtk::prelude::*;

mod app;
mod apply;
mod cli;
mod config;
mod dsl;
mod edit;
mod error_dialog;
mod flake_inputs;
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
    let args = match cli::LaunchArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("nix-settings-gui: {error}");
            return glib::ExitCode::from(2);
        }
    };
    let application = adw::Application::builder()
        .application_id(app::APP_ID)
        .build();
    application.connect_startup(|_| {
        gtk::Window::set_default_icon_name("preferences-system");
    });
    app::run(&application, args.page)
}
