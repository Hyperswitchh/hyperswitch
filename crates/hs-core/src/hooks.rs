//! Display + audio switching via templated host commands.
//!
//! Keeping these as config-driven commands (instead of hard-coding a compositor or
//! audio stack) lets the same daemon drive Sway, Hyprland, cage, PipeWire, etc.
//!
//! Example (Sway + PipeWire):
//! ```toml
//! on_focus = [
//!   "swaymsg '[{window}] focus, fullscreen enable'",
//!   "/usr/lib/hyperswitch/audio.sh unmute {domain}",
//! ]
//! on_blur = ["/usr/lib/hyperswitch/audio.sh mute {domain}"]
//! ```

use crate::config::OsEntry;
use anyhow::{bail, Result};
use std::process::Command;

pub trait Hooks: Send + Sync {
    fn focus(&self, os: &OsEntry) -> Result<()>;
    fn blur(&self, os: &OsEntry) -> Result<()>;
}

pub fn render(template: &str, os: &OsEntry) -> String {
    return template
        .replace("{window}", &os.window_criteria())
        .replace("{monitor_input}", &os.monitor_input())
        .replace("{id}", &os.id)
        .replace("{name}", &os.name)
        .replace("{domain}", &os.domain);
}

/// Runs each command with `sh -c`. Commands run sequentially; keep them fast.
pub struct ShellHooks {
    pub on_focus: Vec<String>,
    pub on_blur: Vec<String>,
}

impl ShellHooks {
    fn run(cmds: &[String], os: &OsEntry) -> Result<()> {
        for t in cmds {
            let cmd = render(t, os);
            let status = Command::new("sh").arg("-c").arg(&cmd).status()?;
            if !status.success() {
                bail!("hook failed ({status}): {cmd}");
            }
        }
        return Ok(());
    }
}

impl Hooks for ShellHooks {
    fn focus(&self, os: &OsEntry) -> Result<()> {
        return Self::run(&self.on_focus, os);
    }
    fn blur(&self, os: &OsEntry) -> Result<()> {
        return Self::run(&self.on_blur, os);
    }
}

/// No-op hooks for tests.
pub struct NoHooks;
impl Hooks for NoHooks {
    fn focus(&self, _: &OsEntry) -> Result<()> {
        return Ok(());
    }
    fn blur(&self, _: &OsEntry) -> Result<()> {
        return Ok(());
    }
}

// === Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DisplayMode;

    fn os(display: DisplayMode) -> OsEntry {
        return OsEntry {
            id: "win11".into(),
            name: "Windows 11".into(),
            domain: "hs-win11".into(),
            autostart: true,
            display,
        };
    }

    #[test]
    fn render_substitutes_window_and_domain() {
        let o = os(DisplayMode::LookingGlass {
            shm: "/dev/kvmfr0".into(),
        });
        assert_eq!(
            render("swaymsg '[{window}] focus' {domain}", &o),
            "swaymsg '[app_id=\"lg-win11\"] focus' hs-win11"
        );
    }

    #[test]
    fn render_substitutes_monitor_input() {
        let o = os(DisplayMode::Passthrough {
            monitor_input: Some("hdmi2".into()),
        });
        assert_eq!(
            render("ddcutil setvcp 60 {monitor_input}", &o),
            "ddcutil setvcp 60 hdmi2"
        );
    }
}
