/// Card surfaces whose DMS defaults differ, and the default geometry the
/// editor uses when a card is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GridSurface {
    ControlCenter,
    Dashboard,
    Weather,
    Generic,
}

pub(crate) struct CardSize {
    pub(crate) w: i64,
    pub(crate) h: i64,
    pub(crate) min_w: i64,
    pub(crate) min_h: i64,
    pub(crate) max_w: i64,
    pub(crate) max_h: i64,
}

fn size(w: i64, h: i64) -> CardSize {
    CardSize {
        w,
        h,
        min_w: 1,
        min_h: 1,
        max_w: 64,
        max_h: 64,
    }
}

fn bounded(w: i64, h: i64, min_w: i64, min_h: i64, max_w: i64, max_h: i64) -> CardSize {
    CardSize {
        w,
        h,
        min_w,
        min_h,
        max_w,
        max_h,
    }
}

/// Default and bounds for a known DMS card id. Unknown ids keep the historic
/// 1x1 scaffold so custom configuration remains addable.
pub(crate) fn card_size(surface: GridSurface, id: &str) -> CardSize {
    match surface {
        GridSurface::ControlCenter => control_center_size(id),
        GridSurface::Dashboard => dashboard_size(id),
        GridSurface::Weather => weather_size(id),
        GridSurface::Generic => size(1, 1),
    }
}

fn control_center_size(id: &str) -> CardSize {
    match id {
        "user" => size(5, 1),
        "settings" | "lock" | "power" | "edit" => size(1, 1),
        "brightnessSlider" | "volumeSlider" | "inputVolumeSlider" => size(4, 1),
        "runningApps" => size(4, 1),
        "quickTiles" => size(4, 1),
        "wifi"
        | "bluetooth"
        | "audioOutput"
        | "audioInput"
        | "battery"
        | "diskUsage"
        | "builtin_vpn"
        | "builtin_cups"
        | "builtin_tailscale"
        | "builtin_display_profiles" => size(4, 1),
        "nightMode" | "darkMode" | "doNotDisturb" | "idleInhibitor" | "colorPicker" => size(2, 1),
        _ => size(4, 1),
    }
}

fn dashboard_size(id: &str) -> CardSize {
    match id {
        "clock" => bounded(2, 2, 1, 1, 64, 64),
        "weather" => bounded(1, 2, 1, 1, 64, 64),
        "notifications" => bounded(3, 5, 3, 2, 64, 64),
        "calendar" => bounded(3, 3, 3, 3, 64, 64),
        "media" => bounded(2, 1, 1, 1, 64, 64),
        "user" => bounded(3, 1, 1, 1, 64, 3),
        "sysmon" => bounded(2, 1, 1, 1, 64, 64),
        "network" => bounded(2, 1, 1, 1, 64, 3),
        "cpu" | "memory" | "disk" | "battery" => bounded(1, 1, 1, 1, 64, 3),
        "wellbeing" => bounded(2, 1, 1, 1, 64, 3),
        _ => size(2, 1),
    }
}

fn weather_size(id: &str) -> CardSize {
    match id {
        "conditions" | "hourly" => bounded(2, 2, 2, 2, 4, 4),
        "daily" => bounded(4, 2, 2, 2, 4, 4),
        "sun" | "moon" => bounded(1, 2, 1, 2, 4, 3),
        "humidity" | "wind" | "pressure" | "precipitation" | "visibility" | "uv" => {
            bounded(1, 2, 1, 2, 2, 3)
        }
        _ => size(1, 2),
    }
}

/// Option defaults scaffolded when a new card is added. Values are raw Nix
/// literals so strings keep their quotes without another serializer here.
pub(crate) fn card_defaults(surface: GridSurface, id: &str) -> Vec<(&'static str, &'static str)> {
    match (surface, id) {
        (GridSurface::ControlCenter, "battery") => {
            vec![
                ("batteryStyle", "\"icon\""),
                ("batteryColorMode", "\"level\""),
            ]
        }
        (GridSurface::ControlCenter, "diskUsage") => {
            vec![("mountPath", "\"/\""), ("showMountPath", "true")]
        }
        (GridSurface::ControlCenter, "idleInhibitor") => vec![("durationMinutes", "0")],
        (GridSurface::ControlCenter, "user") => {
            vec![
                ("background", "false"),
                ("shape", "\"circle\""),
                ("hostname", "true"),
                ("compositor", "true"),
                ("uptime", "true"),
                ("badge", "true"),
            ]
        }
        (GridSurface::Dashboard, "calendar") => vec![("weekends", "false")],
        (GridSurface::Dashboard, "weather") => {
            vec![("city", "false"), ("readings", "true"), ("tone", "\"\"")]
        }
        (GridSurface::Dashboard, "clock") => vec![
            ("style", "\"digital\""),
            ("seconds", "false"),
            ("date", "false"),
            ("numbers", "false"),
            ("tone", "\"primary\""),
        ],
        (GridSurface::Weather, "conditions") => {
            vec![("city", "false"), ("tone", "\"primary\"")]
        }
        (GridSurface::Weather, "hourly") | (GridSurface::Weather, "daily") => {
            vec![("forecast", "\"chart\"")]
        }
        (GridSurface::Weather, "humidity")
        | (GridSurface::Weather, "wind")
        | (GridSurface::Weather, "pressure")
        | (GridSurface::Weather, "precipitation")
        | (GridSurface::Weather, "visibility")
        | (GridSurface::Weather, "uv") => vec![("graphics", "true"), ("tone", "\"\"")],
        (GridSurface::Weather, "sun") | (GridSurface::Weather, "moon") => {
            vec![("tone", "\"\"")]
        }
        _ => Vec::new(),
    }
}

/// Symbolic icon used by the editor when it cannot render the real widget.
pub(crate) fn widget_icon(id: &str) -> &'static str {
    match id {
        "user" => "avatar-default-symbolic",
        "battery" => "battery-symbolic",
        "settings" | "controlCenterButton" => "emblem-system-symbolic",
        "lock" => "system-lock-screen-symbolic",
        "power" => "system-shutdown-symbolic",
        "brightnessSlider" => "display-brightness-symbolic",
        "volumeSlider" | "audioOutput" | "audioInput" | "music" => "audio-volume-high-symbolic",
        "wifi" | "network" | "network_speed_monitor" => "network-wireless-symbolic",
        "bluetooth" => "bluetooth-symbolic",
        "diskUsage" | "disk" => "drive-harddisk-symbolic",
        "clock" => "alarm-symbolic",
        "weather" | "conditions" | "hourly" | "daily" | "sun" | "moon" => "weather-clear-symbolic",
        "clipboard" => "edit-paste-symbolic",
        "cpuUsage" | "cpu" | "sysmon" => "utilities-system-monitor-symbolic",
        "memUsage" | "memory" => "media-flash-symbolic",
        "calendar" => "x-office-calendar-symbolic",
        "notifications" => "preferences-system-notifications-symbolic",
        "doNotDisturb" => "notifications-disabled-symbolic",
        "idleInhibitor" => "preferences-system-time-symbolic",
        "nightMode" => "night-light-symbolic",
        "darkMode" => "weather-clear-night-symbolic",
        "colorPicker" => "color-select-symbolic",
        "runningApps" => "view-app-grid-symbolic",
        "systemTray" | "tray" => "preferences-system-symbolic",
        "wellbeing" => "emblem-favorite-symbolic",
        "humidity" => "weather-showers-symbolic",
        "wind" => "weather-windy-symbolic",
        "pressure" => "weather-severe-alert-symbolic",
        "precipitation" => "weather-showers-scattered-symbolic",
        "visibility" => "view-reveal-symbolic",
        "uv" => "weather-clear-symbolic",
        _ => "application-x-executable-symbolic",
    }
}

pub(crate) fn widget_description(id: &str) -> &'static str {
    match id {
        "user" => "Profile and account",
        "battery" => "Battery and power profile",
        "settings" => "Open settings",
        "lock" => "Lock the session",
        "power" => "Power menu",
        "brightnessSlider" => "Display brightness",
        "volumeSlider" => "Audio volume",
        "audioOutput" => "Output device",
        "audioInput" => "Input device",
        "wifi" => "Network connection",
        "bluetooth" => "Bluetooth devices",
        "diskUsage" => "Filesystem usage",
        "clock" => "Time and date",
        "weather" => "Current conditions",
        "music" => "Now playing",
        "clipboard" => "Clipboard history",
        "cpuUsage" => "CPU usage",
        "memUsage" => "Memory usage",
        "notifications" => "Notification center",
        "calendar" => "Calendar and events",
        "media" => "Media controls",
        "sysmon" => "System monitor",
        "network" => "Network throughput",
        "disk" => "Disk activity",
        "wellbeing" => "Screen time",
        "conditions" => "Current weather",
        "hourly" => "Hourly forecast",
        "daily" => "Daily forecast",
        "sun" => "Sunrise and sunset",
        "moon" => "Moon position",
        "humidity" => "Humidity reading",
        "wind" => "Wind speed",
        "pressure" => "Pressure reading",
        "precipitation" => "Precipitation chance",
        "visibility" => "Visibility distance",
        "uv" => "UV index",
        _ => "DMS widget",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_defaults_match_dms() {
        assert_eq!(card_size(GridSurface::Weather, "conditions").w, 2);
        assert_eq!(card_size(GridSurface::Weather, "hourly").h, 2);
        assert_eq!(card_size(GridSurface::Weather, "daily").w, 4);
        assert_eq!(card_size(GridSurface::Weather, "sun").h, 2);
    }

    #[test]
    fn dashboard_defaults_match_dms() {
        assert_eq!(card_size(GridSurface::Dashboard, "notifications").h, 5);
        assert_eq!(card_size(GridSurface::Dashboard, "weather").h, 2);
        assert_eq!(card_size(GridSurface::Dashboard, "calendar").min_w, 3);
    }

    #[test]
    fn control_center_actions_stay_small() {
        assert_eq!(card_size(GridSurface::ControlCenter, "settings").w, 1);
        assert_eq!(card_size(GridSurface::ControlCenter, "settings").h, 1);
        assert_eq!(card_size(GridSurface::ControlCenter, "user").w, 5);
    }
}
