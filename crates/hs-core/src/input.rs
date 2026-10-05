//! Input routing, the heart of "instant" switching.
//!
//! ```text
//!  physical kbd/mouse ──EVIOCGRAB──▶ Router ──▶ uinput "hyperswitch-ubuntu-0" ──▶ QEMU input-linux ─▶ Ubuntu VM
//!                                     │    └──▶ uinput "hyperswitch-win11-0"  ──▶ QEMU input-linux ─▶ Windows VM
//!                                     └── HotkeyMatcher ──(HotkeyAction)──▶ Switcher
//! ```
//!
//! Every VM is permanently attached to its *own* virtual device, so switching never
//! re-plugs anything inside the guest: the router just changes which virtual device
//! receives events. That is an atomic store, microseconds rather than seconds.

use std::collections::HashSet;

// === Hotkeys

/// What a hotkey asks the switch engine to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Next,
    Prev,
    /// Zero-based index into `Config::oses`.
    Direct(usize),
}

/// Parses "LEFTCTRL+LEFTALT+RIGHT" into a key set.
pub fn parse_combo(s: &str) -> Vec<String> {
    return s
        .split('+')
        .map(|k| k.trim().trim_start_matches("KEY_").to_ascii_uppercase())
        .collect();
}

/// Pure, testable hotkey state machine. Feed it key names without the `KEY_` prefix.
pub struct HotkeyMatcher {
    pressed: HashSet<String>,
    next: Vec<String>,
    prev: Vec<String>,
    direct_mod: Vec<String>,
    max_direct: usize,
}

impl HotkeyMatcher {
    pub fn new(next: &str, prev: &str, direct_modifier: &str, os_count: usize) -> Self {
        return HotkeyMatcher {
            pressed: HashSet::new(),
            next: parse_combo(next),
            prev: parse_combo(prev),
            direct_mod: parse_combo(direct_modifier),
            max_direct: os_count.min(9),
        };
    }

    fn all_down(&self, keys: &[String]) -> bool {
        return keys.iter().all(|k| self.pressed.contains(k));
    }

    /// Returns an action when `key`'s press completes a combo.
    pub fn on_key(&mut self, key: &str, pressed: bool) -> Option<HotkeyAction> {
        let key = key.trim_start_matches("KEY_").to_ascii_uppercase();
        if !pressed {
            self.pressed.remove(&key);
            return None;
        }
        let fresh = self.pressed.insert(key.clone());
        if !fresh {
            return None; // auto-repeat
        }
        if self.next.last() == Some(&key) && self.all_down(&self.next) {
            return Some(HotkeyAction::Next);
        }
        if self.prev.last() == Some(&key) && self.all_down(&self.prev) {
            return Some(HotkeyAction::Prev);
        }
        if self.all_down(&self.direct_mod) {
            if let Ok(n @ 1..=9) = key.parse::<usize>() {
                if n <= self.max_direct {
                    return Some(HotkeyAction::Direct(n - 1));
                }
            }
        }
        return None;
    }

    /// Feeds one batch of key events and returns the first hotkey it completes, with the keys
    /// held at that moment. Every event in the batch still updates the held state, so a
    /// key-up that shares a batch with the combo's last key-down is not lost. Dropping it would
    /// leave that key "held" and make its next press look like auto-repeat.
    pub fn on_batch(&mut self, events: &[(String, bool)]) -> Option<(HotkeyAction, Vec<String>)> {
        let mut found: Option<(HotkeyAction, Vec<String>)> = None;
        for (key, pressed) in events {
            let action = self.on_key(key, *pressed);
            if found.is_none() {
                if let Some(a) = action {
                    found = Some((a, self.held().cloned().collect()));
                }
            }
        }
        return found;
    }

    /// Keys currently held, released on the old target before switching so no key "sticks".
    pub fn held(&self) -> impl Iterator<Item = &String> {
        return self.pressed.iter();
    }
}

// === Router

/// Moves keyboard/mouse focus to a target VM (by index into `Config::oses`).
pub trait InputRouter: Send + Sync {
    fn focus(&self, index: usize);
    fn active(&self) -> usize;
}

/// Router that only records focus. Used in tests and `--mock`.
#[derive(Default)]
pub struct NullRouter(std::sync::atomic::AtomicUsize);

impl InputRouter for NullRouter {
    fn focus(&self, index: usize) {
        self.0.store(index, std::sync::atomic::Ordering::SeqCst);
    }
    fn active(&self) -> usize {
        return self.0.load(std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(feature = "evdev")]
pub mod evdev_router {
    //! Real router: grabs physical devices, mirrors them into one uinput device per VM.
    use super::*;
    use anyhow::{Context, Result};
    use evdev::{
        uinput::VirtualDevice, uinput::VirtualDeviceBuilder, AbsInfo, Device, EventType,
        InputEvent, InputEventKind, Key, UinputAbsSetup,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::Sender,
        Arc, Mutex,
    };

    pub struct EvdevRouter {
        active: Arc<AtomicUsize>,
    }

    impl InputRouter for EvdevRouter {
        fn focus(&self, index: usize) {
            self.active.store(index, Ordering::SeqCst);
        }
        fn active(&self) -> usize {
            return self.active.load(Ordering::SeqCst);
        }
    }

    impl EvdevRouter {
        /// `vm_ids` names the virtual devices (`hyperswitch-<id>-<n>`); a udev rule
        /// symlinks them to `/dev/input/hyperswitch/<id>-<n>` for QEMU's `input-linux`.
        pub fn spawn(
            devices: &[String],
            vm_ids: &[String],
            matcher: HotkeyMatcher,
            actions: Sender<HotkeyAction>,
        ) -> Result<Arc<Self>> {
            let active = Arc::new(AtomicUsize::new(0));
            let matcher = Arc::new(Mutex::new(matcher));

            for (n, path) in devices.iter().enumerate() {
                let mut phys = Device::open(path).with_context(|| format!("open {path}"))?;
                let targets: Vec<VirtualDevice> = vm_ids
                    .iter()
                    .map(|id| mirror(&phys, &format!("hyperswitch-{id}-{n}")))
                    .collect::<Result<_>>()?;
                phys.grab().with_context(|| format!("grab {path}"))?;

                let (active, matcher, actions) = (active.clone(), matcher.clone(), actions.clone());
                std::thread::Builder::new()
                    .name(format!("hs-input-{n}"))
                    .spawn(move || {
                        let mut targets = targets;
                        loop {
                            let events: Vec<InputEvent> = match phys.fetch_events() {
                                Ok(evs) => evs.collect(),
                                Err(e) => {
                                    tracing::error!(%e, "input device lost");
                                    return;
                                }
                            };
                            let cur = active.load(Ordering::SeqCst);
                            let keys: Vec<(String, bool)> = events
                                .iter()
                                .filter_map(|ev| match ev.kind() {
                                    // value 2 is auto-repeat, ignored for matching
                                    InputEventKind::Key(k) if ev.value() != 2 => {
                                        Some((format!("{k:?}"), ev.value() == 1))
                                    }
                                    _ => None,
                                })
                                .collect();
                            let hit = matcher.lock().unwrap().on_batch(&keys);
                            if let Some((action, held)) = hit {
                                // Release everything that was held on the current VM.
                                let ups: Vec<InputEvent> = held
                                    .iter()
                                    .filter_map(|k| key_code(k))
                                    .map(|c| InputEvent::new(EventType::KEY, c, 0))
                                    .collect();
                                let _ = targets[cur].emit(&ups);
                                let _ = actions.send(action);
                                // The batch that completed the combo belongs to no guest. The
                                // physical releases that follow reach the new guest, where the
                                // input core drops them because those keys are not down there.
                                continue;
                            }
                            // Forward the whole synced batch to the active VM only.
                            let _ = targets[active.load(Ordering::SeqCst)].emit(&events);
                        }
                    })?;
            }
            return Ok(Arc::new(EvdevRouter { active }));
        }
    }

    fn mirror(phys: &Device, name: &str) -> Result<VirtualDevice> {
        let mut b = VirtualDeviceBuilder::new()?.name(name);
        if let Some(keys) = phys.supported_keys() {
            b = b.with_keys(keys)?;
        }
        if let Some(rel) = phys.supported_relative_axes() {
            b = b.with_relative_axes(rel)?;
        }
        if let Some(axes) = phys.supported_absolute_axes() {
            let state = phys.get_abs_state()?;
            for axis in axes.iter() {
                let i = state[axis.0 as usize];
                let info =
                    AbsInfo::new(i.value, i.minimum, i.maximum, i.fuzz, i.flat, i.resolution);
                b = b.with_absolute_axis(&UinputAbsSetup::new(axis, info))?;
            }
        }
        return Ok(b.build()?);
    }

    /// Matcher names are `KEY_*` names with the prefix stripped, but mouse buttons keep
    /// their `BTN_*` name, so try the prefixed form first and the bare name second.
    fn key_code(name: &str) -> Option<u16> {
        return format!("KEY_{name}")
            .parse::<Key>()
            .or_else(|_| name.parse::<Key>())
            .ok()
            .map(|k| k.code());
    }

    // === Tests

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn key_code_resolves_keys_and_buttons() {
            assert_eq!(key_code("LEFTCTRL"), Some(Key::KEY_LEFTCTRL.code()));
            assert_eq!(key_code("BTN_LEFT"), Some(Key::BTN_LEFT.code()));
            assert_eq!(key_code("NOT_A_KEY"), None);
        }
    }
}

// === Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> HotkeyMatcher {
        return HotkeyMatcher::new(
            "LEFTCTRL+LEFTALT+RIGHT",
            "LEFTCTRL+LEFTALT+LEFT",
            "LEFTCTRL+LEFTALT",
            3,
        );
    }

    #[test]
    fn next_combo_fires_on_last_key() {
        let mut h = m();
        assert_eq!(h.on_key("KEY_LEFTCTRL", true), None);
        assert_eq!(h.on_key("KEY_LEFTALT", true), None);
        assert_eq!(h.on_key("KEY_RIGHT", true), Some(HotkeyAction::Next));
    }

    #[test]
    fn repeat_does_not_refire() {
        let mut h = m();
        h.on_key("LEFTCTRL", true);
        h.on_key("LEFTALT", true);
        assert_eq!(h.on_key("RIGHT", true), Some(HotkeyAction::Next));
        assert_eq!(h.on_key("RIGHT", true), None);
    }

    #[test]
    fn direct_jump_bounded_by_os_count() {
        let mut h = m();
        h.on_key("LEFTCTRL", true);
        h.on_key("LEFTALT", true);
        assert_eq!(h.on_key("2", true), Some(HotkeyAction::Direct(1)));
        assert_eq!(h.on_key("7", true), None);
    }

    fn batch(keys: &[(&str, bool)]) -> Vec<(String, bool)> {
        return keys.iter().map(|(k, p)| (k.to_string(), *p)).collect();
    }

    #[test]
    fn combo_and_its_release_in_one_batch_fires_every_time() {
        let mut h = m();
        let tap = batch(&[
            ("LEFTCTRL", true),
            ("LEFTALT", true),
            ("RIGHT", true),
            ("RIGHT", false),
            ("LEFTALT", false),
            ("LEFTCTRL", false),
        ]);
        for _ in 0..3 {
            let (action, held) = h.on_batch(&tap).expect("combo should fire");
            assert_eq!(action, HotkeyAction::Next);
            assert_eq!(held.len(), 3);
            assert_eq!(
                h.held().count(),
                0,
                "a release in the batch must be applied"
            );
        }
    }

    #[test]
    fn plain_arrow_is_ignored() {
        let mut h = m();
        assert_eq!(h.on_key("RIGHT", true), None);
    }
}
