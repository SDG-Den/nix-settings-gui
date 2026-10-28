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
    ("density", "Spacing density of the shell interface."),
    ("interfaceStyle", "Visual style of the shell interface."),
    (
        "sectionDividerOpacity",
        "Opacity of the divider lines between sections.",
    ),
    (
        "launcherEnterAction",
        "Action performed when Enter is pressed in the launcher.",
    ),
    (
        "powerMenuFirmwareSetup",
        "Show the firmware setup entry in the power menu.",
    ),
    (
        "tooltipSurfaceColor",
        "Surface colour mode used by tooltips.",
    ),
    (
        "tooltipSurfaceCustomColor",
        "Custom tooltip surface colour, when the surface mode is custom.",
    ),
    ("dotIcon", "Icon shown on the workspace dot indicator."),
    ("dotIconColor", "Colour of the workspace dot icon."),
    ("dotIconImage", "Image used for the workspace dot icon."),
    ("dotIconSymbol", "Symbol used for the workspace dot icon."),
    (
        "dotEdgeMargin",
        "Margin between the workspace dots and the bar edge.",
    ),
    (
        "dotResetPosition",
        "Reset the workspace dot position when the workspace set changes.",
    ),
];

pub(crate) fn field_caption(name: &str) -> String {
    if name == "_order" {
        return "Position".to_string();
    }
    if let Some(label) = super::dms_catalog::settings_label(name) {
        return label.to_string();
    }
    title_case_id(name)
}

/// The one-line captions under the five fields of a menu entry pop-out. These
/// fields are GUI-bespoke, so the help text lives here, not in the template.
pub(crate) const MENU_FIELD_HELP: &[(&str, &str)] = &[
    (
        "provider",
        "Contents source: a provider like apps, custom (run a command), or Open Application (open a path).",
    ),
    ("target", "URL or path opened when the entry is activated."),
    ("action", "Shell command run when the entry is activated."),
    ("title", "Header text while this submenu is open; defaults to the label."),
    ("iconFont", "Font family for the icon glyph when it is not the menu font."),
    (
        "checked",
        "Command whose exit status decides the toggle state.",
    ),
    (
        "when",
        "Command; the entry is shown only while it succeeds.",
    ),
    (
        "disabled",
        "Shell command; the row is dimmed and unselectable while it succeeds.",
    ),
];

/// A choice in the menu pop-out provider dropdown. `label` is both the dropdown
/// text and the stable round-trip key; `provider` is the string written to the
/// config (empty means leave the `provider` key out). `fields` are the main-box
/// fields shown in addition to `provider`, `title` and `iconFont`.
pub(crate) struct MenuProviderChoice {
    pub(crate) label: &'static str,
    pub(crate) provider: &'static str,
    pub(crate) fields: &'static [&'static str],
}

pub(crate) const MENU_PROVIDERS: &[MenuProviderChoice] = &[
    MenuProviderChoice {
        label: "Open Application",
        provider: "",
        fields: &["target"],
    },
    MenuProviderChoice {
        label: "Run Command",
        provider: "",
        fields: &["action"],
    },
    MenuProviderChoice {
        label: "Open Webpage",
        provider: "",
        fields: &["target"],
    },
    MenuProviderChoice {
        label: "Apps List",
        provider: "apps",
        fields: &[],
    },
];

/// Provider labels valid at one menu depth. The applications provider creates
/// a dynamic submenu, so it is only valid on a top-level entry.
pub(crate) fn menu_provider_labels_for(top_level: bool, extra: Option<String>) -> Vec<String> {
    let mut labels: Vec<String> = MENU_PROVIDERS
        .iter()
        .filter(|choice| top_level || choice.provider != "apps")
        .map(|choice| choice.label.to_string())
        .collect();
    if let Some(extra) = extra {
        if top_level || menu_provider_value(&extra) != "apps" {
            labels.push(extra);
        }
    }
    labels
}

pub(crate) fn menu_provider_allowed(label: &str, top_level: bool) -> bool {
    top_level || menu_provider_value(label) != "apps"
}

pub(crate) fn menu_provider_value(label: &str) -> &str {
    MENU_PROVIDERS
        .iter()
        .find(|c| c.label == label)
        .map(|c| c.provider)
        .unwrap_or(label)
}

pub(crate) fn menu_provider_fields(label: &str) -> &'static [&'static str] {
    MENU_PROVIDERS
        .iter()
        .find(|c| c.label == label)
        .map(|c| c.fields)
        .unwrap_or(&[])
}

pub(crate) fn menu_provider_initial(provider: &str, target: &str, action: &str) -> String {
    if !provider.is_empty() {
        return MENU_PROVIDERS
            .iter()
            .find(|c| c.provider == provider)
            .map(|c| c.label.to_string())
            .unwrap_or_else(|| provider.to_string());
    }
    if !target.is_empty() {
        return MENU_PROVIDERS
            .iter()
            .find(|c| c.fields.contains(&"target"))
            .map(|c| c.label.to_string())
            .unwrap_or_else(|| MENU_PROVIDERS[0].label.to_string());
    }
    if !action.is_empty() {
        return MENU_PROVIDERS
            .iter()
            .find(|c| c.fields.contains(&"action"))
            .map(|c| c.label.to_string())
            .unwrap_or_else(|| MENU_PROVIDERS[0].label.to_string());
    }
    "Run Command".to_string()
}

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
        for name in [
            "provider", "target", "action", "title", "iconFont", "checked", "when", "disabled",
        ] {
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

    #[test]
    fn dux_shell_2_settings_have_help() {
        for name in [
            "density",
            "interfaceStyle",
            "sectionDividerOpacity",
            "launcherEnterAction",
            "powerMenuFirmwareSetup",
            "tooltipSurfaceColor",
            "tooltipSurfaceCustomColor",
            "dotIcon",
            "dotIconColor",
            "dotIconImage",
            "dotIconSymbol",
            "dotEdgeMargin",
            "dotResetPosition",
        ] {
            assert!(field_help(name).is_some(), "missing help for {name}");
        }
    }

    #[test]
    fn menu_provider_table_has_unique_labels() {
        let mut seen = std::collections::HashSet::new();
        for choice in MENU_PROVIDERS {
            assert!(seen.insert(choice.label), "duplicate provider label");
            for field in choice.fields {
                assert!(
                    menu_field_help(field).is_some(),
                    "provider field {field} has no help"
                );
            }
        }
        assert!(
            MENU_PROVIDERS.iter().any(|c| c.provider == "apps"),
            "upstream apps provider missing"
        );
    }

    #[test]
    fn apps_provider_is_top_level_only() {
        assert!(menu_provider_labels_for(true, None).contains(&"Apps List".to_string()));
        assert!(!menu_provider_labels_for(false, None).contains(&"Apps List".to_string()));
        assert!(menu_provider_allowed("Apps List", true));
        assert!(!menu_provider_allowed("Apps List", false));
    }

    #[test]
    fn menu_provider_initial_picks_expected_choice() {
        assert_eq!(
            menu_provider_initial("", "https://x", ""),
            "Open Application"
        );
        assert_eq!(menu_provider_initial("", "", "echo hi"), "Run Command");
        assert_eq!(menu_provider_initial("apps", "", ""), "Apps List");
        assert_eq!(menu_provider_initial("future", "", ""), "future");
        assert_eq!(menu_provider_initial("", "", ""), "Run Command");
    }
}
