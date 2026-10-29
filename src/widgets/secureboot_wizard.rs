//! Guided Secure Boot setup. The dialog always derives the next stage from live
//! state, so it can be reopened after a reboot or a firmware change.

use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::context::BuildContext;
use super::secureboot::{badge, StatusView};
use crate::model::Record;
use crate::secureboot::{self, KeyAction, SecureBootState, SetupOptions, SetupStage};

const STAGES: [SetupStage; 7] = [
    SetupStage::Keys,
    SetupStage::Config,
    SetupStage::Rebuild,
    SetupStage::FirmwareSetup,
    SetupStage::Enroll,
    SetupStage::Verify,
    SetupStage::Done,
];

fn key_action(index: u32) -> KeyAction {
    match index {
        1 => KeyAction::Create,
        2 => KeyAction::Migrate,
        _ => KeyAction::Keep,
    }
}

fn stage_position(stage: SetupStage) -> usize {
    STAGES.iter().position(|item| *item == stage).unwrap_or(0)
}

pub(crate) fn open(
    parent: Option<&gtk::Window>,
    record: Record,
    ctx: &BuildContext,
    configured: bool,
    on_done: Rc<dyn Fn()>,
) {
    let dialog = gtk::Dialog::builder()
        .title("Secure Boot setup")
        .modal(true)
        .default_width(760)
        .default_height(780)
        .build();
    if let Some(parent) = parent {
        dialog.set_transient_for(Some(parent));
    }
    dialog.add_button("Close", gtk::ResponseType::Close);

    let scrolled = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    scrolled.set_child(Some(&content));
    dialog.content_area().append(&scrolled);

    let stage_title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-2"])
        .label("Checking state...")
        .build();
    content.append(&stage_title);
    let stage_detail = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();
    content.append(&stage_detail);

    let checklist = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let mut stage_rows: Vec<(SetupStage, gtk::Image, gtk::Label)> = Vec::new();
    for stage in STAGES {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::from_icon_name("radio-symbolic");
        icon.set_pixel_size(16);
        let label = gtk::Label::builder()
            .label(stage.title())
            .xalign(0.0)
            .css_classes(["caption"])
            .build();
        row.append(&icon);
        row.append(&label);
        checklist.append(&row);
        stage_rows.push((stage, icon, label));
    }
    content.append(&checklist);

    let status = StatusView::new();
    content.append(&status.group);

    let options_group = adw::PreferencesGroup::new();
    options_group.set_title("Setup options");

    let key_drop = gtk::DropDown::from_strings(&[
        KeyAction::Keep.label(),
        KeyAction::Create.label(),
        KeyAction::Migrate.label(),
    ]);
    key_drop.set_selected(0);
    let key_row = adw::ActionRow::new();
    key_row.set_title("Keys");
    key_row.set_subtitle("Existing keys are never overwritten");
    key_row.add_suffix(&key_drop);
    options_group.add(&key_row);

    let microsoft = gtk::Switch::new();
    microsoft.set_active(true);
    let microsoft_row = adw::ActionRow::new();
    microsoft_row.set_title("Include Microsoft keys");
    microsoft_row.set_subtitle("Keeps Windows and vendor option ROMs bootable");
    microsoft_row.add_suffix(&microsoft);
    options_group.add(&microsoft_row);

    let firmware_builtin = gtk::Switch::new();
    let firmware_row = adw::ActionRow::new();
    firmware_row.set_title("Include firmware built-in keys");
    firmware_row.set_subtitle("Framework and other vendors that need their own certificates");
    firmware_row.add_suffix(&firmware_builtin);
    options_group.add(&firmware_row);

    let disable_grub = gtk::Switch::new();
    disable_grub.set_active(true);
    let grub_row = adw::ActionRow::new();
    grub_row.set_title("Disable GRUB");
    grub_row.set_subtitle("Lanzaboote replaces systemd-boot; GRUB must not stay enabled");
    grub_row.add_suffix(&disable_grub);
    options_group.add(&grub_row);

    content.append(&options_group);

    let log = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(["caption"])
        .build();
    content.append(&log);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    let refresh_btn = gtk::Button::with_label("Refresh");
    let apply_btn = gtk::Button::with_label("Write configuration");
    let next_btn = gtk::Button::with_label("Next step");
    next_btn.add_css_class("suggested-action");
    buttons.append(&refresh_btn);
    buttons.append(&apply_btn);
    buttons.append(&next_btn);
    content.append(&buttons);

    let state: Rc<RefCell<Option<SecureBootState>>> = Rc::new(RefCell::new(None));
    let configured = Rc::new(Cell::new(configured));
    let written = Rc::new(Cell::new(false));

    let options_for: Rc<dyn Fn() -> SetupOptions> = {
        let key_drop = key_drop.clone();
        let microsoft = microsoft.clone();
        let firmware_builtin = firmware_builtin.clone();
        let disable_grub = disable_grub.clone();
        Rc::new(move || SetupOptions {
            key_action: key_action(key_drop.selected()),
            include_microsoft: microsoft.is_active(),
            include_firmware_builtin: firmware_builtin.is_active(),
            disable_grub: disable_grub.is_active(),
        })
    };

    let update_ui: Rc<dyn Fn()> = {
        let state = state.clone();
        let configured = configured.clone();
        let stage_title = stage_title.clone();
        let stage_detail = stage_detail.clone();
        let stage_rows = stage_rows.clone();
        let key_drop = key_drop.clone();
        let next_btn = next_btn.clone();
        let log = log.clone();
        let options_for = options_for.clone();
        Rc::new(move || {
            let Some(state) = state.borrow().clone() else {
                return;
            };
            if !state.keys_present && key_action(key_drop.selected()) == KeyAction::Keep {
                key_drop.set_selected(1);
            }
            if state.keys_present && key_action(key_drop.selected()) == KeyAction::Create {
                key_drop.set_selected(0);
            }

            let current = secureboot::stage(&state, configured.get());
            stage_title.set_text(current.title());
            stage_detail.set_text(current.detail());
            let position = stage_position(current);
            for (stage, icon, label) in &stage_rows {
                let index = stage_position(*stage);
                if *stage == current {
                    icon.set_icon_name(Some("go-next-symbolic"));
                    label.add_css_class("accent");
                } else {
                    label.remove_css_class("accent");
                    if index < position {
                        icon.set_icon_name(Some("emblem-ok-symbolic"));
                    } else {
                        icon.set_icon_name(Some("radio-symbolic"));
                    }
                }
            }

            let options = options_for();
            let label = match current {
                SetupStage::NotUefi => "UEFI required",
                SetupStage::Keys => options.key_action.label(),
                SetupStage::Config => "Write configuration",
                SetupStage::Rebuild => "Rebuild now",
                SetupStage::FirmwareSetup => "Reboot to firmware",
                SetupStage::Enroll => "Enroll keys",
                SetupStage::Verify => "Rebuild and re-check",
                SetupStage::Done => "Close",
            };
            next_btn.set_label(label);
            next_btn.set_sensitive(current != SetupStage::NotUefi);
            log.set_text(&format!(
                "{} · {}",
                badge(&state, configured.get()),
                stage_detail.text()
            ));
            if let Some(error) = options.validation_error(&state) {
                log.set_text(&error);
            }
        })
    };

    let refresh: Rc<dyn Fn()> = {
        let hook = ctx.secureboot_status.clone();
        let state = state.clone();
        let status = status.clone();
        let update_ui = update_ui.clone();
        let configured = configured.clone();
        let log = log.clone();
        Rc::new(move || {
            let Some(hook) = hook.clone() else {
                log.set_text("Secure Boot support is not available");
                return;
            };
            status.set_checking();
            let state = state.clone();
            let status = status.clone();
            let update_ui = update_ui.clone();
            let configured = configured.clone();
            let log = log.clone();
            hook(
                true,
                Box::new(move |result| {
                    status.update(&result, configured.get());
                    match result {
                        Ok(value) => {
                            *state.borrow_mut() = Some(value);
                        }
                        Err(error) => log.set_text(&error),
                    }
                    update_ui();
                }),
            );
        })
    };

    let write: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let record = record.clone();
        let options_for = options_for.clone();
        let configured = configured.clone();
        let log = log.clone();
        let refresh = refresh.clone();
        let on_done = on_done.clone();
        let written = written.clone();
        let apply_btn = apply_btn.clone();
        Rc::new(move || {
            if written.get() {
                log.set_text("Configuration already written. Reopen the dialog to change it.");
                return;
            }
            let options = options_for();
            let Some(write) = ctx.secureboot_write.clone() else {
                log.set_text("Cannot write the configuration");
                return;
            };
            match write(record.clone(), options) {
                Ok(summary) => {
                    configured.set(true);
                    written.set(true);
                    apply_btn.set_sensitive(false);
                    log.set_text(&summary);
                    refresh();
                    on_done();
                }
                Err(error) => log.set_text(&error),
            }
        })
    };

    {
        let refresh = refresh.clone();
        refresh_btn.connect_clicked(move |_| refresh());
    }
    {
        let write = write.clone();
        apply_btn.connect_clicked(move |_| write());
    }
    {
        let ctx = ctx.clone();
        let state = state.clone();
        let configured = configured.clone();
        let options_for = options_for.clone();
        let log = log.clone();
        let refresh = refresh.clone();
        let write = write.clone();
        let on_done = on_done.clone();
        let dialog = dialog.clone();
        next_btn.connect_clicked(move |_| {
            let Some(current) = state.borrow().clone() else {
                return;
            };
            let options = options_for();
            if let Some(error) = options.validation_error(&current) {
                log.set_text(&error);
                return;
            }
            match secureboot::stage(&current, configured.get()) {
                SetupStage::Keys => {
                    let commands = secureboot::setup_commands(&options, &current);
                    run_commands(&ctx, &commands, &log, &on_done);
                }
                SetupStage::Config => write(),
                SetupStage::Rebuild => {
                    if let Some(rebuild) = &ctx.secureboot_rebuild {
                        rebuild();
                        log.set_text("Rebuild started in the terminal.");
                        on_done();
                    } else {
                        log.set_text("No rebuild command is configured");
                    }
                }
                SetupStage::FirmwareSetup => {
                    let commands = vec!["systemctl reboot --firmware-setup".to_string()];
                    run_commands(&ctx, &commands, &log, &on_done);
                }
                SetupStage::Enroll => {
                    let commands = secureboot::setup_commands(&options, &current);
                    run_commands(&ctx, &commands, &log, &on_done);
                }
                SetupStage::Verify => {
                    if let Some(rebuild) = &ctx.secureboot_rebuild {
                        rebuild();
                        log.set_text("Rebuild started in the terminal. Refresh when it finishes.");
                        on_done();
                    } else {
                        refresh();
                    }
                }
                SetupStage::Done => {
                    dialog.response(gtk::ResponseType::Close);
                }
                SetupStage::NotUefi => {}
            }
        });
    }

    dialog.connect_response(|dialog, _| dialog.close());
    refresh();
    dialog.present();
}

fn run_commands(ctx: &BuildContext, commands: &[String], log: &gtk::Label, on_done: &Rc<dyn Fn()>) {
    if commands.is_empty() {
        log.set_text("There is no privileged action left for this step");
        return;
    }
    let Some(run) = ctx.secureboot_run.clone() else {
        log.set_text("Cannot run privileged commands");
        return;
    };
    match run(commands) {
        Ok(()) => {
            log.set_text(
                "Command started in the terminal. Refresh when it finishes, or reboot if the flow asks for it.",
            );
            on_done();
        }
        Err(error) => log.set_text(&error),
    }
}
