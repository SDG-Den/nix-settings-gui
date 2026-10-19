pub(crate) const WIDGET_NAMES: &[(&str, &str)] = &[
    ("layout", "Layout"),
    ("launcherButton", "App launcher"),
    ("workspaceSwitcher", "Workspace switcher"),
    ("focusedWindow", "Focused window"),
    ("runningApps", "Running apps"),
    ("appsDock", "Apps dock"),
    ("island", "Island"),
    ("clock", "Clock"),
    ("weather", "Weather"),
    ("music", "Media"),
    ("clipboard", "Clipboard manager"),
    ("cpuUsage", "CPU usage"),
    ("memUsage", "Memory usage"),
    ("diskUsage", "Disk usage"),
    ("cpuTemp", "CPU temperature"),
    ("gpuTemp", "GPU temperature"),
    ("systemTray", "System tray"),
    ("privacyIndicator", "Privacy indicator"),
    ("controlCenterButton", "Control Center"),
    ("notificationButton", "Notification Center"),
    ("battery", "Battery"),
    ("vpn", "VPN"),
    ("idleInhibitor", "Idle inhibitor"),
    ("capsLockIndicator", "Caps Lock indicator"),
    ("spacer", "Spacer"),
    ("separator", "Separator"),
    ("network_speed_monitor", "Network speed monitor"),
    ("keyboard_layout_name", "Keyboard layout"),
    ("notepadButton", "Notepad"),
    ("colorPicker", "Color Picker"),
    ("systemUpdate", "System update"),
    ("powerMenuButton", "Power"),
    ("dockLauncher", "Launcher"),
    ("dockTrash", "Trash"),
    ("left", "Left"),
    ("center", "Center"),
    ("right", "Right"),
    ("wifi", "Wi-Fi"),
    ("builtin_vpn", "Built-in VPN"),
    ("builtinVpn", "Built-in VPN"),
    ("uv", "UV"),
];

pub(crate) fn known_widget_name(id: &str) -> Option<&'static str> {
    WIDGET_NAMES
        .iter()
        .find(|(key, _)| *key == id)
        .map(|(_, name)| *name)
}

pub(crate) fn title_case_id(id: &str) -> String {
    let expanded = id.replace('_', " ");
    let mut out = String::new();
    for (index, ch) in expanded.chars().enumerate() {
        if ch.is_uppercase() && index > 0 && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push(ch);
    }
    let mut chars = out.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => out,
    }
}

pub(crate) fn widget_display_name(id: &str) -> String {
    known_widget_name(id)
        .map(str::to_string)
        .unwrap_or_else(|| title_case_id(id))
}

/// The label a keybind row shows: the template's `@table-item` override when
/// one is set, otherwise the derived title-case name, or "New bind" while the
/// name is still empty. A trailing colon is added only when missing.
pub(crate) fn bind_display_label(key: &str, label_override: Option<&str>) -> String {
    let base = match label_override
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        Some(label) => label.to_string(),
        None if key.is_empty() => "New bind".to_string(),
        None => title_case_id(key),
    };
    if base.ends_with(':') || base.ends_with(';') {
        base
    } else {
        format!("{base}:")
    }
}

pub(crate) const WIDGET_FIELD_HELP: &[(&str, &str)] = &[
    ("enabled", "Whether this widget is shown."),
    (
        "_order",
        "Sort position among sibling widgets; lower comes first.",
    ),
    (
        "showOccupiedWorkspacesOnly",
        "Only list workspaces that have open windows.",
    ),
    ("showWorkspaceIndex", "Show the workspace number."),
    (
        "showWorkspacePadding",
        "Pad single-digit workspace numbers.",
    ),
    (
        "workspacePaddingCount",
        "Pad workspace numbers up to this width.",
    ),
    ("dwlShowAllTags", "Show every dwl tag, not only used ones."),
    ("showAudioPercent", "Show the audio level as a percentage."),
    ("showBatteryIcon", "Show the battery icon."),
    ("showBrightnessIcon", "Show the brightness icon."),
    ("showBrightnessPercent", "Show brightness as a percentage."),
    ("showDoNotDisturbIcon", "Show the do not disturb icon."),
    ("showIdleInhibitorIcon", "Show the idle inhibitor icon."),
    ("showMicIcon", "Show the microphone icon."),
    ("showMicPercent", "Show the mic level as a percentage."),
    (
        "autoHide",
        "Hide the bar or dock when a window would overlap it.",
    ),
    ("autoHideDelay", "Milliseconds before auto hide engages."),
    ("position", "Screen edge: top or bottom."),
];

pub(crate) fn field_caption(name: &str) -> String {
    if name == "_order" {
        return "Position".to_string();
    }
    title_case_id(name)
}

/// The one-line captions under the five fields of a menu entry pop-out. These
/// fields are GUI-bespoke, so the help text lives here, not in the template.
pub(crate) const MENU_FIELD_HELP: &[(&str, &str)] = &[
    (
        "provider",
        "Where the entry gets its items, e.g. the apps menu.",
    ),
    ("target", "URL or path opened when the entry is activated."),
    ("action", "Shell command run when the entry is activated."),
    (
        "checked",
        "Command whose exit status decides the toggle state.",
    ),
    (
        "when",
        "Command; the entry is shown only while it succeeds.",
    ),
];

/// The one-line captions under the field editors of a keybind pop-out.
pub(crate) const KEYBIND_FIELD_HELP: &[(&str, &str)] = &[
    (
        "dispatcher",
        "mango action the key triggers, e.g. focusdir, spawn.",
    ),
    ("args", "Argument passed to the dispatcher."),
    ("group", "Coarse category used to sort the main list."),
    ("subcat", "Finer category used within the group."),
];

pub(crate) fn help_text(
    table: &'static [(&'static str, &'static str)],
    name: &str,
) -> Option<&'static str> {
    table
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, help)| *help)
}

pub(crate) fn field_help(name: &str) -> Option<&'static str> {
    help_text(WIDGET_FIELD_HELP, name)
}

pub(crate) fn menu_field_help(name: &str) -> Option<&'static str> {
    help_text(MENU_FIELD_HELP, name)
}

pub(crate) fn keybind_field_help(name: &str) -> Option<&'static str> {
    help_text(KEYBIND_FIELD_HELP, name)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popout_help_covers_every_field() {
        for name in ["provider", "target", "action", "checked", "when"] {
            assert!(
                menu_field_help(name).is_some(),
                "missing menu help for {name}"
            );
        }
        for name in ["dispatcher", "args", "group", "subcat"] {
            assert!(
                keybind_field_help(name).is_some(),
                "missing keybind help for {name}"
            );
        }
    }
}
