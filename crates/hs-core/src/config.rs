//! Configuration model. See `config/hyperswitch.toml` for a full example.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub hotkeys: Hotkeys,
    pub switch: SwitchConfig,
    /// One entry per guest OS, in switch order.
    #[serde(rename = "os")]
    pub oses: Vec<OsEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Hotkeys {
    /// e.g. "LEFTCTRL+LEFTALT+RIGHT"
    pub next: String,
    pub prev: String,
    /// Jump straight to OS #N with modifier + number key (e.g. "LEFTCTRL+LEFTALT").
    pub direct_modifier: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SwitchConfig {
    /// Soft budget; the daemon logs a warning when a switch exceeds it.
    #[serde(default = "default_latency")]
    pub target_latency_ms: u64,
    /// Physical devices the router grabs, e.g. /dev/input/by-id/usb-...-event-kbd
    #[serde(default)]
    pub input_devices: Vec<String>,
    /// Host commands run on switch. `{id}`, `{name}`, `{domain}`, `{window}` (sway
    /// criteria for the guest's viewer) and `{monitor_input}` are substituted.
    #[serde(default)]
    pub on_focus: Vec<String>,
    #[serde(default)]
    pub on_blur: Vec<String>,
}

fn default_latency() -> u64 {
    return 100;
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OsEntry {
    /// Short stable id used by the CLI/UI ("ubuntu", "win11").
    pub id: String,
    /// Display name shown in the switcher overlay.
    pub name: String,
    /// libvirt domain name.
    pub domain: String,
    /// Start this VM when the daemon starts. Keep `true` for instant switching.
    #[serde(default = "yes")]
    pub autostart: bool,
    pub display: DisplayMode,
}

fn yes() -> bool {
    return true;
}

impl OsEntry {
    /// Sway criteria that match this guest's viewer window. Passthrough guests have no
    /// viewer window, so their criteria match nothing and the focus command is a no-op.
    pub fn window_criteria(&self) -> String {
        return match &self.display {
            DisplayMode::LookingGlass { .. } => format!("app_id=\"lg-{}\"", self.id),
            DisplayMode::Spice { .. } => format!("title=\"lg-{}\"", self.id),
            DisplayMode::Passthrough { .. } => format!("title=\"hs-passthrough-{}\"", self.id),
        };
    }

    /// Monitor input to select for passthrough guests, empty for every other mode.
    pub fn monitor_input(&self) -> String {
        return match &self.display {
            DisplayMode::Passthrough { monitor_input } => monitor_input.clone().unwrap_or_default(),
            _ => String::new(),
        };
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DisplayMode {
    /// GPU passed through via VFIO, frames relayed by Looking Glass (IVSHMEM / KVMFR).
    LookingGlass { shm: String },
    /// Virtual GPU (virtio-gpu / QXL) shown via SPICE. Works everywhere, slower.
    Spice { port: u16 },
    /// GPU passed through and wired to its own monitor input.
    Passthrough { monitor_input: Option<String> },
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        return Self::parse(&raw);
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(raw).context("parsing hyperswitch config")?;
        cfg.validate()?;
        return Ok(cfg);
    }

    fn validate(&self) -> Result<()> {
        if self.oses.is_empty() {
            bail!("config must define at least one [[os]] entry");
        }
        let mut seen = HashSet::new();
        for os in &self.oses {
            if !seen.insert(&os.id) {
                bail!("duplicate os id `{}`", os.id);
            }
        }
        return Ok(());
    }

    pub fn find(&self, id: &str) -> Option<&OsEntry> {
        return self.oses.iter().find(|o| o.id == id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const SAMPLE: &str = include_str!("../../../config/hyperswitch.toml");

    #[test]
    fn parses_example_config() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.oses.len(), 2);
        assert_eq!(cfg.oses[0].id, "ubuntu");
        assert!(matches!(
            cfg.oses[1].display,
            DisplayMode::LookingGlass { .. }
        ));
    }

    #[test]
    fn window_criteria_follows_display_mode() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.oses[0].window_criteria(), "title=\"lg-ubuntu\"");
        assert_eq!(cfg.oses[1].window_criteria(), "app_id=\"lg-win11\"");
        assert_eq!(cfg.oses[1].monitor_input(), "");
    }

    #[test]
    fn rejects_duplicate_ids() {
        let dup = SAMPLE.replace("id = \"win11\"", "id = \"ubuntu\"");
        assert!(Config::parse(&dup).is_err());
    }
}
