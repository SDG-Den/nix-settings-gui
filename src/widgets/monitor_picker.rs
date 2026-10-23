use crate::monitor_detect::{DetectedMode, DetectedMonitor};
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;
/// The callback the monitor picker calls with the chosen head and mode, or
/// `None` for the manual scaffold.
pub(crate) type MonitorPick = Rc<dyn Fn(Option<(String, DetectedMode)>)>;

pub(crate) fn open_monitor_picker<F: Fn(Option<(String, DetectedMode)>) + 'static>(
    anchor: &gtk::Widget,
    existing_names: Vec<String>,
    detected: async_channel::Receiver<Option<Vec<DetectedMonitor>>>,
    on_pick: F,
) {
    let on_pick: MonitorPick = Rc::new(on_pick);
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeft);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.append(&stack);

    let busy = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let spinner = gtk::Spinner::new();
    spinner.start();
    busy.append(&spinner);
    let busy_label = gtk::Label::builder()
        .label("Detecting outputs…")
        .css_classes(["dim-label", "caption"])
        .build();
    busy.append(&busy_label);
    stack.add_named(&busy, Some("busy"));
    stack.set_visible_child_name("busy");

    {
        let stack = stack.clone();
        let popover = popover.clone();
        let on_pick = on_pick.clone();
        glib::MainContext::default().spawn_local(async move {
            let outcome = detected.recv().await.ok().flatten();
            match outcome {
                Some(monitors) => {
                    let stage = if monitors.is_empty() {
                        let empty = gtk::Box::new(gtk::Orientation::Vertical, 6);
                        let label = gtk::Label::builder()
                            .label("No outputs detected.")
                            .xalign(0.0)
                            .css_classes(["dim-label", "caption"])
                            .build();
                        empty.append(&label);
                        let custom = gtk::Button::with_label("Add monitor manually");
                        let on_pick_manual = on_pick.clone();
                        let popover_manual = popover.clone();
                        custom.connect_clicked(move |_| {
                            on_pick_manual(None);
                            popover_manual.popdown();
                        });
                        empty.append(&custom);
                        empty
                    } else {
                        build_heads_stage(monitors, &existing_names, &popover, &stack, &on_pick)
                    };
                    if let Some(old) = stack.child_by_name("busy") {
                        stack.remove(&old);
                    }
                    stack.add_named(&stage, Some("heads"));
                    stack.set_visible_child_name("heads");
                }
                None => {
                    let failed = gtk::Box::new(gtk::Orientation::Vertical, 6);
                    let label = gtk::Label::builder()
                        .label("Could not run wlr-randr. Is this a Wayland session?")
                        .xalign(0.0)
                        .css_classes(["dim-label", "caption"])
                        .build();
                    failed.append(&label);
                    let custom = gtk::Button::with_label("Add monitor manually");
                    let on_pick_manual = on_pick.clone();
                    let popover_manual = popover.clone();
                    custom.connect_clicked(move |_| {
                        on_pick_manual(None);
                        popover_manual.popdown();
                    });
                    failed.append(&custom);
                    if let Some(old) = stack.child_by_name("busy") {
                        stack.remove(&old);
                    }
                    stack.add_named(&failed, Some("heads"));
                    stack.set_visible_child_name("heads");
                }
            }
        });
    }
    popover.set_child(Some(&content));
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();
}

fn build_heads_stage(
    detected: Vec<DetectedMonitor>,
    existing_names: &[String],
    popover: &gtk::Popover,
    stack: &gtk::Stack,
    on_pick: &MonitorPick,
) -> gtk::Box {
    let heads = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(false);
    for monitor in detected {
        let configured = existing_names.iter().any(|name| name == &monitor.name);
        let mut text = monitor.name.clone();
        if let (Some(make), Some(model)) = (&monitor.make, &monitor.model) {
            if !make.is_empty() && !model.is_empty() {
                text = format!("{}  {} {}", monitor.name, make, model);
            }
        }
        if !monitor.enabled {
            text = format!("{text} (disabled)");
        }
        if configured {
            text = format!("{text}  (configured)");
        }
        let label = gtk::Label::builder().label(&text).xalign(0.0).build();
        if !monitor.enabled || configured {
            label.add_css_class("dim-label");
        }
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        label.set_margin_start(6);
        label.set_margin_end(6);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&label));
        list.append(&row);
        let stack_activate = stack.clone();
        let on_pick_activate = on_pick.clone();
        let popover_activate = popover.clone();
        let monitor_activate = monitor.clone();
        row.connect_activate(move |_| {
            show_mode_stage(
                &monitor_activate,
                &stack_activate,
                &on_pick_activate,
                &popover_activate,
            );
        });
        let stack_click = stack.clone();
        let on_pick_click = on_pick.clone();
        let popover_click = popover.clone();
        let monitor_click = monitor.clone();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_, n_press, _, _| {
            if n_press == 2 {
                show_mode_stage(&monitor_click, &stack_click, &on_pick_click, &popover_click);
            }
        });
        row.add_controller(click);
    }
    heads.append(&list);
    let custom = gtk::Button::with_label("Add monitor manually");
    let on_pick_manual = on_pick.clone();
    let popover_manual = popover.clone();
    custom.connect_clicked(move |_| {
        on_pick_manual(None);
        popover_manual.popdown();
    });
    heads.append(&custom);
    heads
}

fn show_mode_stage(
    monitor: &DetectedMonitor,
    stack: &gtk::Stack,
    on_pick: &MonitorPick,
    popover: &gtk::Popover,
) {
    if let Some(existing) = stack.child_by_name("modes") {
        stack.remove(&existing);
    }
    let stage = build_monitor_mode_stage(monitor, on_pick, popover, stack);
    stack.add_named(&stage, Some("modes"));
    stack.set_visible_child_name("modes");
}

/// The stage-two page of the monitor picker: choose one advertised mode for a
/// single head, with Back returning to the head list.
pub(crate) fn build_monitor_mode_stage(
    monitor: &DetectedMonitor,
    on_pick: &MonitorPick,
    popover: &gtk::Popover,
    stack: &gtk::Stack,
) -> gtk::Box {
    let stage = gtk::Box::new(gtk::Orientation::Vertical, 6);

    let labels: Vec<String> = monitor
        .modes
        .iter()
        .map(|mode| {
            let mut text = format!("{}x{} @{:.0} Hz", mode.width, mode.height, mode.refresh);
            if mode.preferred {
                text.push_str("  (preferred)");
            }
            if mode.current {
                text.push_str("  (current)");
            }
            text
        })
        .collect();
    let refs: Vec<&str> = labels.iter().map(|label| label.as_str()).collect();
    let combo = gtk::DropDown::from_strings(&refs);
    let initial = monitor
        .default_mode()
        .and_then(|mode| monitor.modes.iter().position(|m| std::ptr::eq(m, mode)))
        .unwrap_or(0);
    if !labels.is_empty() {
        combo.set_selected(initial as u32);
    }
    stage.append(&combo);

    if monitor.modes.is_empty() {
        combo.set_sensitive(false);
        let note = gtk::Label::builder()
            .label("No advertised modes — 1920x1080 @ 60 Hz will be used.")
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();
        stage.append(&note);
    }

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let back = gtk::Button::with_label("Back");
    let add = gtk::Button::with_label("Add");
    add.add_css_class("suggested-action");
    buttons.append(&back);
    buttons.append(&add);
    stage.append(&buttons);

    {
        let stack = stack.clone();
        back.connect_clicked(move |_| stack.set_visible_child_name("heads"));
    }

    {
        let name = monitor.name.clone();
        let modes = monitor.modes.clone();
        let on_pick = on_pick.clone();
        let popover = popover.clone();
        add.connect_clicked(move |_| {
            let mode = modes
                .get(combo.selected() as usize)
                .cloned()
                .unwrap_or(DetectedMode {
                    width: 1920,
                    height: 1080,
                    refresh: 60.0,
                    preferred: false,
                    current: false,
                });
            on_pick(Some((name.clone(), mode)));
            popover.popdown();
        });
    }

    stage
}
