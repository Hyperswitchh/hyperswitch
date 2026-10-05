//! The switch engine.
//!
//! Switch order is chosen so the user *feels* the switch immediately:
//!
//! 1. ensure target VM is running (resume if paused, which is cheap; cold-boot is the slow path)
//! 2. move input        (atomic store, ~µs)
//! 3. blur old display/audio, focus new display/audio (compositor + PipeWire, ~ms)
//!
//! If step 3 fails, input is moved back so keystrokes never go to a guest the screen
//! is not showing. All guests keep running in the background, so nothing boots on a
//! normal switch.

use crate::{
    backend::{BackendError, Hypervisor, VmState},
    config::{Config, OsEntry},
    hooks::Hooks,
    input::{HotkeyAction, InputRouter},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;

// === Types

#[derive(Debug, Error)]
pub enum SwitchError {
    #[error("unknown os `{0}`")]
    UnknownOs(String),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("hook error: {0}")]
    Hook(String),
}

/// Microseconds spent in each step of a switch. `input_us`, `blur_us` and `focus_us` are zero
/// when the target is already the active OS.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct StepTimes {
    pub state_us: u64,
    pub input_us: u64,
    pub blur_us: u64,
    pub focus_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SwitchReport {
    pub from: String,
    pub to: String,
    pub elapsed_us: u64,
    /// True when the target had to be cold-booted (slow path).
    pub cold_start: bool,
    pub over_budget: bool,
    #[serde(default)]
    pub steps: StepTimes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsStatus {
    pub id: String,
    pub name: String,
    pub state: VmState,
    pub active: bool,
}

pub struct Switcher {
    cfg: Config,
    hv: Arc<dyn Hypervisor>,
    input: Arc<dyn InputRouter>,
    hooks: Arc<dyn Hooks>,
}

fn micros(since: Instant) -> u64 {
    return since.elapsed().as_micros() as u64;
}

// === Engine

impl Switcher {
    pub fn new(
        cfg: Config,
        hv: Arc<dyn Hypervisor>,
        input: Arc<dyn InputRouter>,
        hooks: Arc<dyn Hooks>,
    ) -> Self {
        return Switcher {
            cfg,
            hv,
            input,
            hooks,
        };
    }

    /// Boot every `autostart` OS so later switches are instant.
    pub fn boot_all(&self) -> Result<(), SwitchError> {
        for os in self.cfg.oses.iter().filter(|o| o.autostart) {
            if self.hv.state(&os.domain)? != VmState::Running {
                tracing::info!(os = %os.id, "starting");
                self.hv.start(&os.domain)?;
            }
        }
        let first = &self.cfg.oses[0];
        // The display session starts after the daemon, so the first focus often has no
        // window to target yet. That must not stop the daemon from coming up.
        if let Err(e) = self.hooks.focus(first) {
            tracing::warn!(os = %first.id, %e, "initial focus hook failed, continuing");
        }
        self.input.focus(0);
        return Ok(());
    }

    pub fn active(&self) -> &OsEntry {
        return &self.cfg.oses[self.input.active()];
    }

    pub fn status(&self) -> Vec<OsStatus> {
        let active = self.input.active();
        return self
            .cfg
            .oses
            .iter()
            .enumerate()
            .map(|(i, o)| OsStatus {
                id: o.id.clone(),
                name: o.name.clone(),
                state: self.hv.state(&o.domain).unwrap_or(VmState::Unknown),
                active: i == active,
            })
            .collect();
    }

    pub fn handle(&self, action: HotkeyAction) -> Result<SwitchReport, SwitchError> {
        let n = self.cfg.oses.len();
        let cur = self.input.active();
        let idx = match action {
            HotkeyAction::Next => (cur + 1) % n,
            HotkeyAction::Prev => (cur + n - 1) % n,
            HotkeyAction::Direct(i) => i.min(n - 1),
        };
        return self.switch_index(idx);
    }

    pub fn switch_to(&self, id: &str) -> Result<SwitchReport, SwitchError> {
        let idx = self
            .cfg
            .oses
            .iter()
            .position(|o| o.id == id)
            .ok_or_else(|| SwitchError::UnknownOs(id.into()))?;
        return self.switch_index(idx);
    }

    fn switch_index(&self, idx: usize) -> Result<SwitchReport, SwitchError> {
        let t0 = Instant::now();
        let from_idx = self.input.active();
        let from = self.cfg.oses[from_idx].clone();
        let to = &self.cfg.oses[idx];
        let mut steps = StepTimes::default();

        // 1. make sure the target is live
        let t = Instant::now();
        let mut cold_start = false;
        match self.hv.state(&to.domain)? {
            VmState::Running => {}
            VmState::Paused => self.hv.resume(&to.domain)?,
            _ => {
                cold_start = true;
                self.hv.start(&to.domain)?;
            }
        }
        steps.state_us = micros(t);

        if from.id != to.id {
            // 2. input first, so the user's next keystroke lands in the new OS
            let t = Instant::now();
            self.input.focus(idx);
            steps.input_us = micros(t);
            // 3. display + audio
            match self.apply_hooks(&from, to) {
                Ok((blur_us, focus_us)) => {
                    steps.blur_us = blur_us;
                    steps.focus_us = focus_us;
                }
                Err(e) => {
                    self.input.focus(from_idx);
                    if let Err(restore) = self.hooks.focus(&from) {
                        tracing::warn!(os = %from.id, %restore, "could not restore focus after failed switch");
                    }
                    return Err(SwitchError::Hook(e.to_string()));
                }
            }
        }

        let elapsed = t0.elapsed();
        let over_budget =
            !cold_start && elapsed > Duration::from_millis(self.cfg.switch.target_latency_ms);
        if over_budget {
            tracing::warn!(
                ?elapsed,
                budget_ms = self.cfg.switch.target_latency_ms,
                "switch over latency budget"
            );
        }
        return Ok(SwitchReport {
            from: from.id,
            to: to.id.clone(),
            elapsed_us: elapsed.as_micros() as u64,
            cold_start,
            over_budget,
            steps,
        });
    }

    /// Returns the microseconds spent in the blur hook and the focus hook.
    fn apply_hooks(&self, from: &OsEntry, to: &OsEntry) -> anyhow::Result<(u64, u64)> {
        let t = Instant::now();
        self.hooks.blur(from)?;
        let blur_us = micros(t);
        let t = Instant::now();
        self.hooks.focus(to)?;
        let focus_us = micros(t);
        return Ok((blur_us, focus_us));
    }

    pub fn start(&self, id: &str) -> Result<(), SwitchError> {
        let os = self
            .cfg
            .find(id)
            .ok_or_else(|| SwitchError::UnknownOs(id.into()))?;
        return Ok(self.hv.start(&os.domain)?);
    }

    pub fn stop(&self, id: &str) -> Result<(), SwitchError> {
        let os = self
            .cfg
            .find(id)
            .ok_or_else(|| SwitchError::UnknownOs(id.into()))?;
        return Ok(self.hv.shutdown(&os.domain)?);
    }
}

// === Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backend::MockHypervisor, hooks::NoHooks, input::NullRouter};
    use anyhow::bail;

    /// Fails `focus` for one OS id, succeeds for everything else.
    struct FailFocusFor(&'static str);

    impl Hooks for FailFocusFor {
        fn focus(&self, os: &OsEntry) -> anyhow::Result<()> {
            if os.id == self.0 {
                bail!("no window for {}", os.id);
            }
            return Ok(());
        }
        fn blur(&self, _: &OsEntry) -> anyhow::Result<()> {
            return Ok(());
        }
    }

    fn setup_with(hooks: Arc<dyn Hooks>) -> Switcher {
        let cfg = Config::parse(include_str!("../../../config/hyperswitch.toml")).unwrap();
        let hv = Arc::new(MockHypervisor::with_domains(
            cfg.oses.iter().map(|o| o.domain.as_str()),
        ));
        return Switcher::new(cfg, hv, Arc::new(NullRouter::default()), hooks);
    }

    fn setup() -> Switcher {
        return setup_with(Arc::new(NoHooks));
    }

    #[test]
    fn boot_then_cycle() {
        let s = setup();
        s.boot_all().unwrap();
        assert_eq!(s.active().id, "ubuntu");
        let r = s.handle(HotkeyAction::Next).unwrap();
        assert_eq!(
            (r.from.as_str(), r.to.as_str(), r.cold_start),
            ("ubuntu", "win11", false)
        );
        let r = s.handle(HotkeyAction::Next).unwrap();
        assert_eq!(r.to, "ubuntu"); // wraps
    }

    #[test]
    fn cold_start_is_flagged() {
        let s = setup();
        let r = s.switch_to("win11").unwrap();
        assert!(r.cold_start);
    }

    #[test]
    fn unknown_os_errors() {
        assert!(matches!(
            setup().switch_to("macos"),
            Err(SwitchError::UnknownOs(_))
        ));
    }

    #[test]
    fn boot_survives_initial_focus_failure() {
        let s = setup_with(Arc::new(FailFocusFor("ubuntu")));
        assert!(s.boot_all().is_ok());
        assert_eq!(s.active().id, "ubuntu");
    }

    #[test]
    fn steps_are_reported_and_fit_inside_the_total() {
        let s = setup();
        s.boot_all().unwrap();
        let r = s.handle(HotkeyAction::Next).unwrap();
        let sum = r.steps.state_us + r.steps.input_us + r.steps.blur_us + r.steps.focus_us;
        assert!(
            sum <= r.elapsed_us,
            "steps {sum} us exceed total {} us",
            r.elapsed_us
        );
    }

    #[test]
    fn switching_to_the_active_os_skips_input_and_hooks() {
        let s = setup();
        s.boot_all().unwrap();
        let r = s.switch_to("ubuntu").unwrap();
        assert_eq!(
            (r.steps.input_us, r.steps.blur_us, r.steps.focus_us),
            (0, 0, 0)
        );
    }

    #[test]
    fn failed_hook_rolls_input_back() {
        let s = setup_with(Arc::new(FailFocusFor("win11")));
        s.boot_all().unwrap();
        let err = s.switch_to("win11").unwrap_err();
        assert!(matches!(err, SwitchError::Hook(_)));
        assert_eq!(s.active().id, "ubuntu");
    }
}
