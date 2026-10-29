//! Secure Boot state, setup planning, and Nix configuration generation.
//!
//! The module is split in two layers. The parsers and the setup model are pure
//! and tested against captured tool output. `collect_state` runs `bootctl` and
//! `sbctl`; `app::secureboot` is the only caller that also writes files.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// The firmware Secure Boot mode reported by `bootctl` and `sbctl`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FirmwareMode {
    #[default]
    Unknown,
    Disabled,
    Setup,
    User,
}

impl FirmwareMode {
    pub fn parse(value: &str) -> Self {
        let lower = value.trim().to_ascii_lowercase();
        if lower.contains("enabled") {
            if lower.contains("setup") {
                Self::Setup
            } else {
                Self::User
            }
        } else if lower.contains("disabled") {
            Self::Disabled
        } else {
            Self::Unknown
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Disabled => "disabled",
            Self::Setup => "enabled (setup mode)",
            Self::User => "enabled (user mode)",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BootctlStatus {
    pub uefi: bool,
    pub firmware: Option<String>,
    pub secure_boot: FirmwareMode,
    pub boot_into_firmware: bool,
}

fn field(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some((key, value.trim()))
}

pub fn parse_bootctl_status(text: &str) -> BootctlStatus {
    let mut status = BootctlStatus::default();
    for line in text.lines() {
        let Some((key, value)) = field(line) else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "firmware" => {
                status.uefi = value.to_ascii_lowercase().contains("uefi");
                status.firmware = Some(value.to_string());
            }
            "secure boot" => status.secure_boot = FirmwareMode::parse(value),
            "boot into fw" => {
                status.boot_into_firmware = value.to_ascii_lowercase().contains("supported")
            }
            _ => {}
        }
    }
    status
}

fn clean_value(value: &str) -> String {
    value
        .trim()
        .trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, '✓' | '✗' | '✔' | '✘' | '•' | '*')
        })
        .trim()
        .to_string()
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SbctlStatus {
    pub installed: bool,
    pub owner_guid: Option<String>,
    pub setup_mode: Option<bool>,
    pub secure_boot: Option<bool>,
    pub vendor_keys: Vec<String>,
}

pub fn parse_sbctl_status(text: &str) -> SbctlStatus {
    let mut status = SbctlStatus::default();
    for line in text.lines() {
        let Some((key, value)) = field(line) else {
            continue;
        };
        let value = clean_value(value);
        let lower = value.to_ascii_lowercase();
        match key.to_ascii_lowercase().as_str() {
            "installed" => status.installed = lower.contains("installed"),
            "owner guid" => {
                let empty = value
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == '-' || c.is_whitespace())
                    && value.chars().all(|c| c == '0' || c == '-');
                if !value.is_empty() && !empty {
                    status.owner_guid = Some(value);
                }
            }
            "setup mode" => status.setup_mode = Some(lower.contains("enabled")),
            "secure boot" => status.secure_boot = Some(lower.contains("enabled")),
            "vendor keys" => {
                status.vendor_keys = value
                    .split([',', ' '])
                    .map(|item| item.trim().to_ascii_lowercase())
                    .filter(|item| !item.is_empty() && item != "none")
                    .collect();
            }
            _ => {}
        }
    }
    status
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub signed: Vec<String>,
    pub unsigned: Vec<String>,
}

impl VerifyReport {
    /// Kernel and initrd copies under `EFI/nixos` are expected to be unsigned;
    /// Lanzaboote embeds their hash in the signed stub instead.
    pub fn unsigned_boot_files(&self) -> Vec<&str> {
        self.unsigned
            .iter()
            .map(String::as_str)
            .filter(|path| !path.contains("/EFI/nixos/"))
            .collect()
    }
}

pub fn parse_sbctl_verify(text: &str) -> VerifyReport {
    let mut report = VerifyReport::default();
    for line in text.lines() {
        let trimmed = line.trim();
        let (signed, rest) = if let Some(rest) = trimmed.strip_prefix('✓') {
            (true, rest)
        } else if let Some(rest) = trimmed.strip_prefix('✗') {
            (false, rest)
        } else {
            continue;
        };
        let path = rest
            .split(" is ")
            .next()
            .map(str::trim)
            .filter(|path| !path.is_empty());
        let Some(path) = path else {
            continue;
        };
        if signed {
            report.signed.push(path.to_string());
        } else {
            report.unsigned.push(path.to_string());
        }
    }
    report
}

/// The live state shown by the widget.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecureBootState {
    pub efi: bool,
    pub firmware_mode: FirmwareMode,
    pub firmware: Option<String>,
    pub boot_into_firmware: bool,
    pub sbctl_installed: bool,
    pub owner_guid: Option<String>,
    pub setup_mode_enabled: Option<bool>,
    pub vendor_keys: Vec<String>,
    pub esp_path: Option<String>,
    pub esp_free_bytes: Option<u64>,
    pub key_bundle: String,
    pub keys_present: bool,
    pub lanzaboote_active: bool,
    pub signed_files: usize,
    pub unsigned_files: usize,
    pub windows_present: bool,
    pub bitlocker_present: bool,
    pub errors: Vec<String>,
}

impl SecureBootState {
    pub fn secure_boot_enabled(&self) -> bool {
        matches!(self.firmware_mode, FirmwareMode::User)
    }

    pub fn in_setup_mode(&self) -> bool {
        self.setup_mode_enabled
            .unwrap_or(matches!(self.firmware_mode, FirmwareMode::Setup))
    }
}

fn run(tool: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(tool)
        .args(args)
        .output()
        .or_else(|_| {
            Command::new(format!("/run/current-system/sw/bin/{tool}"))
                .args(args)
                .output()
        })
        .map_err(|error| format!("cannot run {tool}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("{tool} exited with {}", output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn tool_exists(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn esp_free_bytes(esp: &Path) -> Option<u64> {
    let output = Command::new("df")
        .args(["-B1", "--output=avail"])
        .arg(esp)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .nth(1)
        .and_then(|line| line.trim().parse().ok())
}

fn lanzaboote_on_esp(esp: &Path) -> bool {
    let efi_linux = esp.join("EFI").join("Linux");
    let Ok(entries) = std::fs::read_dir(efi_linux) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        name.starts_with("nixos-generation-") && name.ends_with(".efi")
    })
}

fn bitlocker_present() -> bool {
    let Ok(output) = Command::new("lsblk").args(["-pno", "FSTYPE"]).output() else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.to_ascii_lowercase().contains("bitlocker"))
}

/// Collect the live state. Failures are recorded in `errors` and leave the
/// corresponding field unknown rather than failing the whole card.
pub fn collect_state() -> SecureBootState {
    let mut state = SecureBootState {
        efi: Path::new("/sys/firmware/efi").exists(),
        key_bundle: DEFAULT_PKI_BUNDLE.to_string(),
        ..SecureBootState::default()
    };

    if let Ok(text) = run("bootctl", &["status"]) {
        let status = parse_bootctl_status(&text);
        state.firmware = status.firmware;
        state.firmware_mode = status.secure_boot;
        state.boot_into_firmware = status.boot_into_firmware;
        state.efi = state.efi && status.uefi;
    } else {
        state.errors.push("bootctl is not available".to_string());
    }

    state.sbctl_installed = tool_exists("sbctl");
    if state.sbctl_installed {
        match run("sbctl", &["status"]) {
            Ok(text) => {
                let status = parse_sbctl_status(&text);
                if status.secure_boot.is_some() {
                    state.firmware_mode = if status.secure_boot == Some(true) {
                        FirmwareMode::User
                    } else {
                        FirmwareMode::Disabled
                    };
                }
                state.setup_mode_enabled = status.setup_mode;
                if status.setup_mode == Some(true) {
                    state.firmware_mode = FirmwareMode::Setup;
                }
                state.vendor_keys = status.vendor_keys;
                state.owner_guid = status.owner_guid;
            }
            Err(error) => state.errors.push(format!("sbctl status: {error}")),
        }
    }

    let esp = run("bootctl", &["--print-esp-path"])
        .ok()
        .map(|path| PathBuf::from(path.trim()))
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| PathBuf::from("/boot"));
    state.esp_free_bytes = esp_free_bytes(&esp);
    state.esp_path = Some(esp.display().to_string());
    state.lanzaboote_active = lanzaboote_on_esp(&esp);
    state.windows_present = esp
        .join("EFI")
        .join("Microsoft")
        .join("Boot")
        .join("bootmgfw.efi")
        .is_file();

    let keys = Path::new(DEFAULT_PKI_BUNDLE).join("keys");
    state.keys_present = keys.is_dir() || state.owner_guid.is_some();

    if state.sbctl_installed {
        if let Ok(text) = run("sbctl", &["verify"]) {
            let report = parse_sbctl_verify(&text);
            state.signed_files = report.signed.len();
            state.unsigned_files = report.unsigned_boot_files().len();
        }
    }

    state.bitlocker_present = bitlocker_present();
    state
}

pub const DEFAULT_PKI_BUNDLE: &str = "/var/lib/sbctl";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyAction {
    #[default]
    Keep,
    Create,
    Migrate,
}

impl KeyAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Keep => "Keep existing keys",
            Self::Create => "Generate new keys",
            Self::Migrate => "Migrate /etc/secureboot",
        }
    }
}

/// The flake input path used by the generated module. The widget targets
/// dendritic flakes, so `inputs` is always in scope for the generated import.
pub const LANZABOOTE_IMPORT: &str = "inputs.lanzaboote.nixosModules.lanzaboote";

#[derive(Debug, Clone, PartialEq)]
pub struct SetupOptions {
    pub key_action: KeyAction,
    pub include_microsoft: bool,
    pub include_firmware_builtin: bool,
    pub disable_grub: bool,
}

impl Default for SetupOptions {
    fn default() -> Self {
        Self {
            key_action: KeyAction::Keep,
            include_microsoft: true,
            include_firmware_builtin: false,
            disable_grub: true,
        }
    }
}

impl SetupOptions {
    pub fn validation_error(&self, state: &SecureBootState) -> Option<String> {
        if !state.efi {
            return Some("This machine is not booted in UEFI mode".to_string());
        }
        if self.key_action == KeyAction::Create && state.keys_present {
            return Some(format!(
                "Keys already exist in {}. Back them up and remove them first, or keep them.",
                state.key_bundle
            ));
        }
        if self.key_action == KeyAction::Migrate && !Path::new("/etc/secureboot").is_dir() {
            return Some(
                "/etc/secureboot does not exist, so there is nothing to migrate".to_string(),
            );
        }
        if !self.include_microsoft && !state.vendor_keys.is_empty() {
            return Some(
                "Without the Microsoft keys, Windows and some option ROMs may stop booting"
                    .to_string(),
            );
        }
        None
    }
}

/// The next step in the setup flow. Derived from live state so the wizard can
/// resume after a reboot or a firmware change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStage {
    NotUefi,
    Keys,
    Config,
    Rebuild,
    FirmwareSetup,
    Enroll,
    Verify,
    Done,
}

impl SetupStage {
    pub fn title(self) -> &'static str {
        match self {
            Self::NotUefi => "UEFI required",
            Self::Keys => "Prepare keys",
            Self::Config => "Write configuration",
            Self::Rebuild => "Rebuild NixOS",
            Self::FirmwareSetup => "Enter firmware setup mode",
            Self::Enroll => "Enroll keys",
            Self::Verify => "Verify boot files",
            Self::Done => "Secure Boot is active",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::NotUefi => "Secure Boot needs UEFI. Switch the firmware from legacy boot first.",
            Self::Keys => "Create a key bundle in /var/lib/sbctl, or migrate /etc/secureboot.",
            Self::Config => "Add the Lanzaboote input and write the NixOS configuration.",
            Self::Rebuild => "Apply the configuration so Lanzaboote installs signed boot files.",
            Self::FirmwareSetup => {
                "Reboot into firmware settings, enable Secure Boot, and reset to setup mode."
            }
            Self::Enroll => "Enroll your keys, including the Microsoft vendor keys for Windows.",
            Self::Verify => "Reboot, then check that every boot file is signed and enforced.",
            Self::Done => "The firmware enforces Secure Boot and Lanzaboote signed the boot files.",
        }
    }
}

pub fn stage(state: &SecureBootState, configured: bool) -> SetupStage {
    if !state.efi {
        return SetupStage::NotUefi;
    }
    if !state.keys_present {
        return SetupStage::Keys;
    }
    if !configured {
        return SetupStage::Config;
    }
    if !state.lanzaboote_active {
        return SetupStage::Rebuild;
    }
    if state.secure_boot_enabled() {
        return if state.unsigned_files > 0 {
            SetupStage::Verify
        } else {
            SetupStage::Done
        };
    }
    if state.in_setup_mode() {
        return SetupStage::Enroll;
    }
    SetupStage::FirmwareSetup
}

/// The privileged commands for the current options and stage.
pub fn setup_commands(options: &SetupOptions, state: &SecureBootState) -> Vec<String> {
    let mut commands = Vec::new();
    match options.key_action {
        KeyAction::Create if !state.keys_present => {
            commands.push("sh -c 'test ! -d /var/lib/sbctl/keys && sbctl create-keys'".to_string())
        }
        KeyAction::Migrate => commands
            .push("sh -c 'test ! -d /var/lib/sbctl/keys && sbctl setup --migrate'".to_string()),
        _ => {}
    }
    if stage(state, true) == SetupStage::Enroll {
        let mut command = String::from("sbctl enroll-keys");
        if options.include_microsoft {
            command.push_str(" --microsoft");
        } else {
            command.push_str(" --yes-this-might-brick-my-machine");
        }
        if options.include_firmware_builtin {
            command.push_str(" --firmware-builtin");
        }
        commands.push(command);
    }
    commands
}

pub fn nix_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${");
    format!("\"{escaped}\"")
}

/// The config block the wizard writes into the attached module. The app splits
/// this into edits for assignments that already exist in the file. The enabled
/// toggle is the attached `boot.lanzaboote.enable` option and is written by the
/// normal control edit, so this block only adds the remaining options.
#[allow(dead_code)]
pub fn config_lines() -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("imports = [ {LANZABOOTE_IMPORT} ];"));
    lines.push("boot.loader.systemd-boot.enable = lib.mkForce false;".to_string());
    lines.push(format!(
        "boot.lanzaboote.pkiBundle = {};",
        nix_string(DEFAULT_PKI_BUNDLE)
    ));
    lines.push("environment.systemPackages = [ pkgs.sbctl ];".to_string());
    lines
}

#[allow(dead_code)]
pub fn config_block() -> String {
    config_lines().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bootctl_user_mode() {
        let text = "System:\n      Firmware: UEFI 2.70 (Lenovo 0.4720)\n   Secure Boot: enabled (user)\n  Boot into FW: supported\n";
        let status = parse_bootctl_status(text);
        assert!(status.uefi);
        assert_eq!(status.secure_boot, FirmwareMode::User);
        assert!(status.boot_into_firmware);
    }

    #[test]
    fn parses_bootctl_disabled() {
        let status = parse_bootctl_status("Secure Boot: disabled\n");
        assert_eq!(status.secure_boot, FirmwareMode::Disabled);
    }

    #[test]
    fn parses_sbctl_status_with_marks() {
        let text = "Installed:      ✓ sbctl is installed\nOwner GUID:     8ec4b2c3-dc7f-4362-b9a3-0cc17e5a34cd\nSetup Mode:     ✓ Disabled\nSecure Boot:    ✓ Enabled\nVendor Keys:    microsoft\n";
        let status = parse_sbctl_status(text);
        assert!(status.installed);
        assert_eq!(status.setup_mode, Some(false));
        assert_eq!(status.secure_boot, Some(true));
        assert_eq!(status.vendor_keys, vec!["microsoft".to_string()]);
        assert!(status.owner_guid.is_some());
    }

    #[test]
    fn treats_a_zero_owner_guid_as_no_keys() {
        let status = parse_sbctl_status("Owner GUID: 00000000-0000-0000-0000-000000000000\n");
        assert_eq!(status.owner_guid, None);
    }

    #[test]
    fn parses_verify_and_ignores_kernel_copies() {
        let text = "✓ /boot/EFI/BOOT/BOOTX64.EFI is signed\n✗ /boot/EFI/nixos/kernel.efi is not signed\n✗ /boot/EFI/Linux/nixos-generation-1.efi is not signed\n";
        let report = parse_sbctl_verify(text);
        assert_eq!(report.signed.len(), 1);
        assert_eq!(report.unsigned.len(), 2);
        assert_eq!(report.unsigned_boot_files().len(), 1);
    }

    fn state(keys: bool, configured_active: bool, mode: FirmwareMode) -> SecureBootState {
        SecureBootState {
            efi: true,
            keys_present: keys,
            lanzaboote_active: configured_active,
            firmware_mode: mode,
            ..SecureBootState::default()
        }
    }

    #[test]
    fn stage_follows_keys_config_and_firmware() {
        assert_eq!(
            stage(&state(false, false, FirmwareMode::Disabled), false),
            SetupStage::Keys
        );
        assert_eq!(
            stage(&state(true, false, FirmwareMode::Disabled), false),
            SetupStage::Config
        );
        assert_eq!(
            stage(&state(true, true, FirmwareMode::Disabled), true),
            SetupStage::FirmwareSetup
        );
        let mut setup = state(true, true, FirmwareMode::Setup);
        setup.setup_mode_enabled = Some(true);
        assert_eq!(stage(&setup, true), SetupStage::Enroll);
        assert_eq!(
            stage(&state(true, true, FirmwareMode::User), true),
            SetupStage::Done
        );
    }

    #[test]
    fn not_uefi_blocks_the_flow() {
        let mut machine = state(true, true, FirmwareMode::User);
        machine.efi = false;
        assert_eq!(stage(&machine, true), SetupStage::NotUefi);
        let options = SetupOptions::default();
        assert!(options.validation_error(&machine).is_some());
    }

    #[test]
    fn never_generates_keys_when_a_bundle_exists() {
        let options = SetupOptions {
            key_action: KeyAction::Create,
            ..SetupOptions::default()
        };
        let commands = setup_commands(&options, &state(true, true, FirmwareMode::Setup));
        assert!(!commands
            .iter()
            .any(|command| command.contains("create-keys")));
        assert!(options
            .validation_error(&state(true, true, FirmwareMode::Setup))
            .is_some());
    }

    #[test]
    fn enrolls_microsoft_keys_by_default() {
        let mut machine = state(true, true, FirmwareMode::Setup);
        machine.setup_mode_enabled = Some(true);
        let commands = setup_commands(&SetupOptions::default(), &machine);
        assert!(commands
            .iter()
            .any(|command| command == "sbctl enroll-keys --microsoft"));
    }

    #[test]
    fn generated_config_parses_as_a_nix_attrset() {
        let block = config_block();
        let wrapped = format!("{{ {block} }}");
        assert!(rnix::Root::parse(&wrapped).ok().is_ok());
        assert!(block.contains("boot.lanzaboote.pkiBundle = \"/var/lib/sbctl\";"));
        assert!(block.contains("inputs.lanzaboote.nixosModules.lanzaboote"));
    }

    #[test]
    fn escapes_nix_strings() {
        assert_eq!(nix_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(nix_string("a${b}"), "\"a\\${b}\"");
    }
}
