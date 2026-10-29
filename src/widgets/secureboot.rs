//! The Secure Boot card: live firmware state, the Lanzaboote toggle, and the
//! entry point to the guided setup flow.

use adw::prelude::*;
use std::rc::Rc;

use super::context::BuildContext;
use super::editor::Editor;
use super::nix::truthy;
use crate::model::Record;
use crate::secureboot::SecureBootState;

#[derive(Clone)]
pub(crate) struct StatusView {
    pub(crate) group: adw::PreferencesGroup,
    summary: adw::ActionRow,
    firmware: adw::ActionRow,
    secure_boot: adw::ActionRow,
    setup: adw::ActionRow,
    keys: adw::ActionRow,
    vendor: adw::ActionRow,
    loader: adw::ActionRow,
    esp: adw::ActionRow,
    windows: adw::ActionRow,
    bitlocker: adw::ActionRow,
    errors: adw::ActionRow,
}

impl Default for StatusView {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusView {
    pub(crate) fn new() -> Self {
        let group = adw::PreferencesGroup::new();
        group.set_title("Secure Boot state");
        group.set_hexpand(true);

        let summary = adw::ActionRow::new();
        summary.set_title("Status");
        summary.set_subtitle("Checking...");
        let firmware = row("Firmware");
        let secure_boot = row("Secure Boot");
        let setup = row("Setup mode");
        let keys = row("Key bundle");
        let vendor = row("Vendor keys");
        let loader = row("Boot loader");
        let esp = row("EFI system partition");
        let windows = row("Windows");
        let bitlocker = row("BitLocker");
        let errors = row("Problems");
        windows.set_visible(false);
        bitlocker.set_visible(false);
        errors.set_visible(false);
        for item in [
            &summary,
            &firmware,
            &secure_boot,
            &setup,
            &keys,
            &vendor,
            &loader,
            &esp,
            &windows,
            &bitlocker,
            &errors,
        ] {
            group.add(item);
        }
        Self {
            group,
            summary,
            firmware,
            secure_boot,
            setup,
            keys,
            vendor,
            loader,
            esp,
            windows,
            bitlocker,
            errors,
        }
    }

    pub(crate) fn set_checking(&self) {
        self.summary.set_subtitle("Checking...");
    }

    pub(crate) fn update(&self, result: &Result<SecureBootState, String>, configured: bool) {
        match result {
            Err(error) => {
                self.summary.set_subtitle("Unavailable");
                self.errors.set_subtitle(error);
                self.errors.set_visible(true);
                self.firmware.set_visible(false);
                self.secure_boot.set_visible(false);
                self.setup.set_visible(false);
                self.keys.set_visible(false);
                self.vendor.set_visible(false);
                self.loader.set_visible(false);
                self.esp.set_visible(false);
                self.windows.set_visible(false);
                self.bitlocker.set_visible(false);
            }
            Ok(state) => {
                self.summary.set_subtitle(&badge(state, configured));
                self.firmware.set_visible(true);
                self.firmware.set_subtitle(&format!(
                    "{}{}",
                    if state.efi { "UEFI" } else { "legacy BIOS" },
                    state
                        .firmware
                        .as_deref()
                        .map(|value| format!(" · {value}"))
                        .unwrap_or_default()
                ));
                self.secure_boot.set_visible(true);
                self.secure_boot.set_subtitle(state.firmware_mode.label());
                self.setup.set_visible(true);
                self.setup.set_subtitle(bool_label(state.in_setup_mode()));
                self.keys.set_visible(true);
                self.keys.set_subtitle(&key_label(state));
                self.vendor.set_visible(true);
                self.vendor.set_subtitle(&if state.vendor_keys.is_empty() {
                    "none".to_string()
                } else {
                    state.vendor_keys.join(", ")
                });
                self.loader.set_visible(true);
                self.loader.set_subtitle(&if state.lanzaboote_active {
                    if state.signed_files == 0 && state.unsigned_files == 0 {
                        "Lanzaboote detected on the ESP".to_string()
                    } else {
                        format!(
                            "Lanzaboote · {} signed, {} unsigned",
                            state.signed_files, state.unsigned_files
                        )
                    }
                } else {
                    "Lanzaboote not detected on the ESP".to_string()
                });
                self.esp.set_visible(true);
                self.esp.set_subtitle(&esp_label(state));
                self.windows.set_visible(state.windows_present);
                self.windows
                    .set_subtitle("Windows boot manager found on the ESP");
                self.bitlocker.set_visible(state.bitlocker_present);
                self.bitlocker.set_subtitle(
                    "BitLocker volume found. Save the recovery key before enrolling keys.",
                );
                self.errors.set_visible(!state.errors.is_empty());
                self.errors.set_subtitle(&state.errors.join("; "));
            }
        }
    }
}

fn row(title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.set_subtitle("unknown");
    row.set_activatable(false);
    row
}

fn bool_label(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn key_label(state: &SecureBootState) -> String {
    let mut text = if state.keys_present {
        format!("present in {}", state.key_bundle)
    } else {
        format!("no bundle in {}", state.key_bundle)
    };
    if let Some(guid) = &state.owner_guid {
        let short: String = guid.chars().take(8).collect();
        text.push_str(&format!(" · owner {short}"));
    }
    text
}

fn esp_label(state: &SecureBootState) -> String {
    let path = state.esp_path.as_deref().unwrap_or("/boot");
    match state.esp_free_bytes {
        Some(bytes) => format!("{path} · {} free", human_bytes(bytes)),
        None => path.to_string(),
    }
}

fn human_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.1} GiB", value / GIB)
    } else {
        format!("{:.0} MiB", value / MIB)
    }
}

pub(crate) fn badge(state: &SecureBootState, configured: bool) -> String {
    if !state.efi {
        return "UEFI required".to_string();
    }
    if !configured {
        return "Disabled in the configuration".to_string();
    }
    if state.secure_boot_enabled() && state.lanzaboote_active {
        if state.unsigned_files > 0 {
            return "Enforced, unsigned files found".to_string();
        }
        return "Enabled and enforced".to_string();
    }
    if state.lanzaboote_active {
        if state.in_setup_mode() {
            return "Ready to enroll keys".to_string();
        }
        return "Installed, Secure Boot disabled in firmware".to_string();
    }
    if state.in_setup_mode() {
        return "Setup mode, rebuild required".to_string();
    }
    "Configured, rebuild required".to_string()
}

fn warning_icon_name() -> &'static str {
    "dialog-warning-symbolic"
}

pub(crate) fn build_secureboot(record: &Record, ctx: &BuildContext) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_hexpand(true);

    let hint = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .label(
            "Lanzaboote signs the boot loader and kernels. Keep Secure Boot disabled \
             in the firmware until the setup flow reaches enrollment.",
        )
        .css_classes(["dim-label", "caption"])
        .build();
    root.append(&hint);

    let correct_path = record.path() == "boot.lanzaboote.enable";
    if !correct_path {
        let error = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .label("Attach this widget to boot.lanzaboote.enable. Setup writes that generic NixOS option.")
            .css_classes(["error"])
            .build();
        root.append(&error);
    }

    let toggle = gtk::Switch::new();
    toggle.set_active(truthy(&record.value));
    toggle.set_valign(gtk::Align::Center);
    toggle.set_halign(gtk::Align::End);

    let toggle_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let toggle_label = gtk::Label::builder()
        .label("Enable Lanzaboote in the configuration")
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    toggle_row.append(&toggle_label);
    toggle_row.append(&toggle);
    root.append(&toggle_row);

    let status = StatusView::new();
    root.append(&status.group);

    let warnings = gtk::Box::new(gtk::Orientation::Vertical, 4);
    warnings.set_visible(false);
    root.append(&warnings);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_halign(gtk::Align::Start);
    let refresh = gtk::Button::with_label("Refresh");
    refresh.add_css_class("flat");
    let setup = gtk::Button::with_label("Set up Secure Boot");
    setup.add_css_class("suggested-action");
    setup.set_sensitive(correct_path);
    actions.append(&refresh);
    actions.append(&setup);
    root.append(&actions);

    let last_state: Rc<std::cell::RefCell<Option<Result<SecureBootState, String>>>> =
        Rc::new(std::cell::RefCell::new(None));
    let update: Rc<dyn Fn(Result<SecureBootState, String>)> = {
        let status = status.clone();
        let warnings = warnings.clone();
        let toggle = toggle.clone();
        let last_state = last_state.clone();
        Rc::new(move |result| {
            *last_state.borrow_mut() = Some(result.clone());
            let active = toggle.is_active();
            if let Ok(state) = &result {
                fill_warnings(&warnings, state, active);
            }
            status.update(&result, active);
        })
    };

    if let Some(hook) = ctx.secureboot_status.clone() {
        status.set_checking();
        let update = update.clone();
        hook(false, Box::new(move |result| update(result)));
    } else {
        update(Err("Secure Boot support is not available".to_string()));
    }

    {
        let update = update.clone();
        let last_state = last_state.clone();
        toggle.connect_active_notify(move |_| {
            if let Some(result) = last_state.borrow().clone() {
                update(result);
            }
        });
    }

    {
        let hook = ctx.secureboot_status.clone();
        let update = update.clone();
        refresh.connect_clicked(move |_| {
            if let Some(hook) = &hook {
                let update = update.clone();
                hook(true, Box::new(move |result| update(result)));
            }
        });
    }

    {
        let record = record.clone();
        let ctx = ctx.clone();
        let update = update.clone();
        let toggle = toggle.clone();
        setup.connect_clicked(move |button| {
            let parent = button.root().and_downcast::<gtk::Window>();
            let on_done: Rc<dyn Fn()> = {
                let hook = ctx.secureboot_status.clone();
                let update = update.clone();
                Rc::new(move || {
                    if let Some(hook) = &hook {
                        hook(
                            true,
                            Box::new({
                                let update = update.clone();
                                move |result| update(result)
                            }),
                        );
                    }
                })
            };
            super::secureboot_wizard::open(
                parent.as_ref(),
                record.clone(),
                &ctx,
                toggle.is_active(),
                on_done,
            );
        });
    }

    Editor::SecureBoot { root, toggle }
}

fn fill_warnings(warnings: &gtk::Box, state: &SecureBootState, configured: bool) {
    while let Some(child) = warnings.first_child() {
        warnings.remove(&child);
    }
    let mut messages = Vec::new();
    if configured && state.secure_boot_enabled() && !state.lanzaboote_active {
        messages.push(
            "Secure Boot is enforced but Lanzaboote is not installed yet. Do not reboot before the rebuild completes.",
        );
    }
    if !configured && state.secure_boot_enabled() {
        messages.push(
            "Secure Boot is enforced. Disable it in the firmware before turning Lanzaboote off.",
        );
    }
    if state.windows_present && !state.vendor_keys.iter().any(|key| key == "microsoft") {
        messages.push(
            "Windows is installed but the Microsoft keys are not enrolled. Enroll them to keep Windows bootable.",
        );
    }
    if state.bitlocker_present {
        messages.push(
            "BitLocker is present. Confirm the recovery key in Windows before changing Secure Boot keys.",
        );
    }
    warnings.set_visible(!messages.is_empty());
    for message in messages {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::from_icon_name(warning_icon_name());
        icon.set_pixel_size(16);
        let label = gtk::Label::builder()
            .label(message)
            .xalign(0.0)
            .wrap(true)
            .hexpand(true)
            .css_classes(["caption"])
            .build();
        row.append(&icon);
        row.append(&label);
        warnings.append(&row);
    }
}
