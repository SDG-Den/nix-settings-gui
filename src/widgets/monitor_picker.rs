use crate::monitor_detect::{DetectedMode, DetectedMonitor};
use gtk::prelude::*;
use std::rc::Rc;
/// The callback the monitor picker calls with the chosen head and mode, or
/// `None` for the manual scaffold.
pub(crate) type MonitorPick = Rc<dyn Fn(Option<(String, DetectedMode)>)>;

pub(crate) fn open_monitor_picker<F: Fn(Option<(String, DetectedMode)>) + 'static>(
    anchor: &gtk::Widget,
    detected: Vec<DetectedMonitor>,
    on_pick: F,
) {
    if detected.is_empty() {
        on_pick(None);
        return;
    }
    let on_pick: MonitorPick = Rc::new(on_pick);
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeft);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.append(&stack);

    let heads = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    for monitor in detected {
        let mut text = monitor.name.clone();
        if let (Some(make), Some(model)) = (&monitor.make, &monitor.model) {
            if !make.is_empty() && !model.is_empty() {
                text = format!("{}  {} {}", monitor.name, make, model);
            }
        }
        if !monitor.enabled {
            text = format!("{text} (disabled)");
        }
        let label = gtk::Label::builder().label(&text).xalign(0.0).build();
        if !monitor.enabled {
            label.add_css_class("dim-label");
        }
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        label.set_margin_start(6);
        label.set_margin_end(6);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&label));
        list.append(&row);
        let stack = stack.clone();
        let on_pick = on_pick.clone();
        let popover = popover.clone();
        row.connect_activate(move |_| {
            if let Some(existing) = stack.child_by_name("modes") {
                stack.remove(&existing);
            }
            let stage = build_monitor_mode_stage(&monitor, &on_pick, &popover, &stack);
            stack.add_named(&stage, Some("modes"));
            stack.set_visible_child_name("modes");
        });
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
    stack.add_named(&heads, Some("heads"));
    stack.set_visible_child_name("heads");

    popover.set_child(Some(&content));
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();
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
    let caption = gtk::Label::builder()
        .label(&monitor.name)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    stage.append(&caption);

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

    let preview = gtk::Label::builder().xalign(0.0).build();
    stage.append(&preview);

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

    let update_preview = {
        let combo = combo.clone();
        let preview = preview.clone();
        let modes = monitor.modes.clone();
        move || {
            if let Some(mode) = modes.get(combo.selected() as usize) {
                preview.set_label(&format!(
                    "{}x{} @ {:.0} Hz",
                    mode.width, mode.height, mode.refresh
                ));
            }
        }
    };
    update_preview();
    combo.connect_selected_notify({
        let update_preview = update_preview.clone();
        move |_| update_preview()
    });

    {
        let name = monitor.name.clone();
        let modes = monitor.modes.clone();
        let on_pick = on_pick.clone();
        let popover = popover.clone();
        add.connect_clicked(move |_| {
            if let Some(mode) = modes.get(combo.selected() as usize) {
                on_pick(Some((name.clone(), mode.clone())));
            }
            popover.popdown();
        });
    }

    stage
}
