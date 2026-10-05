#![cfg(feature = "evdev")]
//! Hardware test for the evdev router. It creates a fake keyboard through `/dev/uinput`,
//! never a real device, and needs access to `/dev/uinput` plus the event nodes it makes,
//! so it is ignored by default. Setup and run command are in plan.md, Phase 4.
//!
//! Run: cargo test -p hs-core --features evdev --test uinput_router -- --ignored --nocapture

use evdev::{
    uinput::{VirtualDevice, VirtualDeviceBuilder},
    AttributeSet, Device, EventType, InputEvent, InputEventKind, Key,
};
use hs_core::input::{evdev_router::EvdevRouter, HotkeyAction, HotkeyMatcher, InputRouter};
use std::{
    collections::HashMap,
    sync::mpsc,
    time::{Duration, Instant},
};

const CYCLES: usize = 100;

// === Helpers

fn settle() {
    std::thread::sleep(Duration::from_millis(30));
}

fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn send(fake: &mut VirtualDevice, key: Key, value: i32) {
    fake.emit(&[InputEvent::new(EventType::KEY, key.code(), value)])
        .expect("emit on fake keyboard");
}

fn tap(fake: &mut VirtualDevice, key: Key) {
    send(fake, key, 1);
    send(fake, key, 0);
}

/// Grabbed before any key is injected, so the desktop never sees these events. A thread
/// owns the device and forwards its key events, since evdev 0.12 has no non-blocking mode.
fn open_sink(name: &str) -> mpsc::Receiver<(u16, i32)> {
    let mut dev = wait_for(name, || {
        evdev::enumerate()
            .map(|(_, d)| d)
            .find(|d| d.name() == Some(name))
    });
    dev.grab().expect("grab sink");
    let (tx, rx) = mpsc::channel::<(u16, i32)>();
    std::thread::spawn(move || {
        while let Ok(events) = dev.fetch_events() {
            for ev in events {
                if let InputEventKind::Key(k) = ev.kind() {
                    if tx.send((k.code(), ev.value())).is_err() {
                        return;
                    }
                }
            }
        }
    });
    return rx;
}

fn drain(rx: &mpsc::Receiver<(u16, i32)>) -> Vec<(u16, i32)> {
    return rx.try_iter().collect();
}

/// Down minus up per key code. A positive count at the end means a key is stuck down.
fn record(tally: &mut HashMap<u16, i64>, events: &[(u16, i32)]) {
    for (code, value) in events {
        match value {
            1 => *tally.entry(*code).or_default() += 1,
            0 => *tally.entry(*code).or_default() -= 1,
            _ => {}
        }
    }
}

fn stuck(tally: &HashMap<u16, i64>) -> Vec<u16> {
    return tally
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(c, _)| *c)
        .collect();
}

// === Test

#[test]
#[ignore = "needs /dev/uinput access, see plan.md Phase 4"]
fn hotkey_switching_routes_input_and_leaves_no_stuck_keys() {
    let mut keys = AttributeSet::<Key>::new();
    for k in [
        Key::KEY_A,
        Key::KEY_LEFTCTRL,
        Key::KEY_LEFTALT,
        Key::KEY_RIGHT,
        Key::KEY_LEFT,
    ] {
        keys.insert(k);
    }
    let mut fake = VirtualDeviceBuilder::new()
        .expect("open /dev/uinput")
        .name("hs-fake-kbd")
        .with_keys(&keys)
        .expect("keys")
        .build()
        .expect("build fake keyboard");
    let path = wait_for("fake keyboard node", || {
        fake.enumerate_dev_nodes_blocking().ok()?.next()?.ok()
    });
    wait_for("fake keyboard readable", || {
        Device::open(&path).ok().map(|_| ())
    });

    let matcher = HotkeyMatcher::new(
        "LEFTCTRL+LEFTALT+RIGHT",
        "LEFTCTRL+LEFTALT+LEFT",
        "LEFTCTRL+LEFTALT",
        2,
    );
    let (tx, rx) = mpsc::channel::<HotkeyAction>();
    let ids = vec!["a".to_string(), "b".to_string()];
    let router = EvdevRouter::spawn(&[path.to_string_lossy().to_string()], &ids, matcher, tx)
        .expect("spawn router");

    let sinks = [open_sink("hyperswitch-a-0"), open_sink("hyperswitch-b-0")];
    let mut tallies: [HashMap<u16, i64>; 2] = [HashMap::new(), HashMap::new()];

    for cycle in 0..CYCLES {
        let cur = router.active();
        let next = (cur + 1) % 2;

        // a plain key must reach only the active guest
        tap(&mut fake, Key::KEY_A);
        settle();
        for i in 0..2 {
            let events = drain(&sinks[i]);
            record(&mut tallies[i], &events);
            let got = events.iter().any(|(c, _)| *c == Key::KEY_A.code());
            assert_eq!(got, i == cur, "cycle {cycle}: A routed to the wrong guest");
        }

        // the hotkey fires an action; the engine would then move focus
        send(&mut fake, Key::KEY_LEFTCTRL, 1);
        send(&mut fake, Key::KEY_LEFTALT, 1);
        send(&mut fake, Key::KEY_RIGHT, 1);
        let action = rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|_| panic!("cycle {cycle}: no hotkey action"));
        assert_eq!(action, HotkeyAction::Next);
        router.focus(next);

        // physical releases after the switch must not reach the new guest
        send(&mut fake, Key::KEY_RIGHT, 0);
        send(&mut fake, Key::KEY_LEFTALT, 0);
        send(&mut fake, Key::KEY_LEFTCTRL, 0);
        settle();

        let old = drain(&sinks[cur]);
        record(&mut tallies[cur], &old);
        let new = drain(&sinks[next]);
        assert!(
            new.is_empty(),
            "cycle {cycle}: new guest saw leftover keys {new:?}"
        );
    }

    for (i, tally) in tallies.iter().enumerate() {
        assert!(
            stuck(tally).is_empty(),
            "guest {i} has stuck keys {:?}",
            stuck(tally)
        );
    }
}
