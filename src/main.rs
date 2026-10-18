#![allow(deprecated)]

use gtk::glib;

mod app;
mod config;
mod dsl;
mod edit;
#[allow(dead_code)]
mod metadata;
#[allow(dead_code)]
mod model;
mod monitor_detect;
mod nixd;
mod scan;
#[allow(dead_code)]
mod templates;
#[allow(dead_code)]
mod value;
mod widgets;

fn main() -> glib::ExitCode {
    let application = adw::Application::builder()
        .application_id(app::APP_ID)
        .build();
    app::run(&application)
}
