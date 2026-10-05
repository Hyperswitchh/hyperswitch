# Hyperswitch Architecture

## 1. Goal

Run several operating systems **at the same time** on one machine and move the
user between them with one hotkey in **under 100 ms**, with no reboot and no lost
state.

The rule behind every design choice:

> **Never boot, pause or re-plug anything on a switch. All guests stay running;
> only the user's keyboard, mouse, screen and audio move.**

## 2. Layer model

```
┌──────────────────────────────────────────────────────────────────────┐
│  Guest: Ubuntu          Guest: Windows 11        Guest: …             │  ring 0 (VMX non-root)
├──────────────────────────────────────────────────────────────────────┤
│  Hyperswitch host userspace                                          │
│   hyperswitchd ── input router · switch engine · IPC socket          │
│   sway (kiosk) ── Looking Glass / SPICE windows, one per guest        │
│   libvirtd + QEMU (one process per guest) · PipeWire                 │
├──────────────────────────────────────────────────────────────────────┤
│  Host Linux kernel: KVM · VFIO · KVMFR · uinput · evdev              │  ring 0 (VMX root, "ring -1")
├──────────────────────────────────────────────────────────────────────┤
│  systemd-boot (EFI partition)                                         │
├──────────────────────────────────────────────────────────────────────┤
│  UEFI firmware: VT-x/AMD-V + VT-d/AMD-Vi enabled                      │
├──────────────────────────────────────────────────────────────────────┤
│  Hardware: CPU · RAM · GPU 0 (host) · GPU 1 (passthrough) · NVMe      │
└──────────────────────────────────────────────────────────────────────┘
```

Hyperswitch is a **minimal Debian-based distro** whose only job is to run this
stack. The firmware's only involvement is enabling virtualization and IOMMU and
then loading the bootloader; Hyperswitch never writes to firmware.

## 3. Boot sequence

```
power on → UEFI → systemd-boot → host kernel (intel_iommu=on iommu=pt)
  → vfio-pci claims the passthrough GPU (modprobe softdep)
  → udev creates /dev/kvmfr0, /dev/uinput
  → libvirtd
  → hyperswitchd
       1. hyperswitch-check (ExecStartPre), abort on FAIL
       2. load /etc/hyperswitch/hyperswitch.toml
       3. grab physical kbd/mouse, create uinput devices per guest
       4. start every autostart guest  (the only slow step, happens once)
       5. focus guest #0, open control socket
  → hyperswitch-session: sway kiosk with one fullscreen viewer per guest
```

From the user's point of view the machine boots straight into their first OS.

## 4. Components

| Component | Crate / file | Responsibility |
|---|---|---|
| Config | `hs-core::config` | Parse and validate TOML; ordered OS list |
| Backend | `hs-core::backend` | `Hypervisor` trait; `LibvirtHypervisor` (prod), `MockHypervisor` (tests) |
| Input router | `hs-core::input` | Grab devices, detect hotkeys, forward events to the active guest |
| Hooks | `hs-core::hooks` | Templated shell commands for display + audio focus/blur |
| Switch engine | `hs-core::switcher` | Orchestrates a switch, measures latency |
| IPC | `hs-core::ipc` | JSON-lines protocol over `/run/hyperswitch/hyperswitch.sock` |
| Daemon | `hs-daemon` (`hyperswitchd`) | Wires it all together; systemd service |
| CLI | `hs-cli` (`hyperswitch`) | `status`, `switch`, `next`, `prev`, `start`, `stop` |
| HW check | `hs-hwcheck` (`hyperswitch-check`) | Pre-install / pre-start capability check |
| Image | `distro/` | mkosi image, systemd units, udev rules, sway kiosk |
| Guests | `vm-templates/` | libvirt domain XML with pinning, hugepages, VFIO, evdev |

Every hardware-facing piece sits behind a trait, so the switch logic is fully
unit-tested without KVM.

## 5. The switch path

```
key press ─▶ input thread ─▶ HotkeyMatcher ─▶ release held keys on old guest
                                          └─▶ mpsc ─▶ Switcher::handle
                                                         │
     1. target state?  Running → ok · Paused → resume · Off → cold boot (flagged)
     2. input.focus(idx)            atomic store           ~1 µs
     3. hooks.blur(old)             PipeWire mute          ~5 ms
     4. hooks.focus(new)            sway focus + unmute    ~10-20 ms
                                                         │
                                         SwitchReport { elapsed_us, cold_start }
```

**Input moves first,** so the user's next keystroke already lands in the new OS
while the screen catches up. Anything slower than `target_latency_ms` is logged. The step timings above are estimates until they are measured on hardware (see `plan.md`, Phase 6).

## 6. Input design

```
/dev/input/by-id/…-kbd ──EVIOCGRAB──▶ hyperswitchd ──▶ uinput "hyperswitch-ubuntu-0" ──▶ QEMU (Ubuntu)
                                           │      └──▶ uinput "hyperswitch-win11-0"  ──▶ QEMU (Windows)
                                           └─ only the active target receives events
```

- The daemon owns the physical devices exclusively (`EVIOCGRAB`), so neither the
  host nor an inactive guest ever sees input.
- Each guest is **permanently** attached to its own virtual device (libvirt
  `<input type="evdev">`), so a switch never hot-plugs hardware in the guest.
  That is what makes it instant and keeps Windows from "device connected" chimes.
- Before switching, every held key gets a key-up on the old guest, so modifiers
  never stick.
- Hotkey matching is a pure state machine (`HotkeyMatcher`), tested in isolation.

## 7. Display modes

| Mode | When | Performance | How |
|---|---|---|---|
| `looking-glass` | Second GPU available | Near-native, ~1 frame latency | VFIO GPU → KVMFR shared memory → `looking-glass-client` |
| `passthrough` | Guest GPU wired to its own monitor input | Native | Hook switches monitor input (DDC/CI `ddcutil`) |
| `spice` | Single GPU / laptops | Good for desktop work, weak for games | virtio-gpu + `remote-viewer` |

All viewers stay open in the sway kiosk; a switch only changes which one is
focused and fullscreen.

## 8. Audio

Each QEMU process is a PipeWire client named after its domain. On switch:
mute the old guest's stream, unmute the new one (`audio.sh`). Guests keep
playing audio internally, so nothing restarts.

## 9. IPC protocol

```
→ {"cmd":"status"}
← {"ok":true,"data":{"status":[{"id":"ubuntu","name":"Ubuntu 24.04","state":"running","active":true}, …]}}
→ {"cmd":"switch","id":"win11"}
← {"ok":true,"data":{"switched":{"from":"ubuntu","to":"win11","elapsed_us":412,"cold_start":false,"over_budget":false}}}
```

Commands: `status`, `switch`, `next`, `prev`, `start`, `stop`. The future overlay
UI (Tauri) uses the same socket.

## 10. Performance tuning (per guest)

- **CPU pinning:** dedicated cores per guest; QEMU emulator threads on host cores.
- **Hugepages:** fewer TLB misses for guest RAM.
- **virtio** disk and network, `cache=none io=native`.
- **`iommu=pt`:** no translation overhead for host devices.
- Input threads run at `SCHED_FIFO` priority (systemd unit).

## 11. Security model

- Guests are isolated by KVM + IOMMU; a passed-through GPU can only DMA into its
  own guest's memory.
- Only the daemon touches physical input; guests receive input only while focused,
  so a background guest cannot keylog the foreground one.
- The control socket is root-owned (`/run/hyperswitch`, mode 0700 via
  `RuntimeDirectoryMode`). Add a group if the UI must run unprivileged.
- No network listener. Updates are signed packages (future).

## 12. Failure handling

| Failure | Behaviour |
|---|---|
| Guest crashes | `status` shows `crashed`; switching to it cold-boots it |
| Daemon crashes | systemd restarts it; guests keep running (owned by libvirtd) |
| Input device unplugged | input thread logs and exits; replug needs a restart (v0.2: hotplug via udev monitor) |
| Hook command fails | input is moved back to the old guest and the switch returns an error, so input and screen never disagree |
| No IOMMU / one GPU | `hyperswitch-check` warns; config falls back to `spice` |

## 13. Roadmap

- **v0.1 (this scaffold):** daemon, CLI, hwcheck, image recipe, Windows template.
- **v0.2:** Tauri overlay (Alt-Tab style), input hotplug, shared clipboard.
- **v0.3:** graphical installer (hwcheck → GPU selection → VM import), virtiofs shared folder.
- **v0.4:** laptop support (SR-IOV / GVT-g iGPU splitting), resource monitor, snapshots.
