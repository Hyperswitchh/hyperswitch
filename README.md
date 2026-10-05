# Hyperswitch

**Run Ubuntu, Windows and more at the same time on one PC, and switch between them instantly with a hotkey.**

Hyperswitch is a minimal Linux distro built around KVM. Every OS keeps running in
the background; pressing `Ctrl+Alt+→` moves your keyboard, mouse, screen and audio
to the next one in milliseconds. No reboots, no lost work.

```
Ctrl+Alt+→  ─▶  hyperswitchd  ─▶  input · display · audio  ─▶  Windows 11   (≈20 ms)
```

> **Status:** v0.1 scaffold. The switch engine, CLI, hardware check and IPC are
> working and tested. The distro image and real-hardware paths still need testing
> on physical machines.

## How it works

```
 Ubuntu VM      Windows VM      …
     ▲              ▲
     └─ hyperswitchd (input router + switch engine) ─ sway kiosk ─ PipeWire
                 KVM · VFIO · Looking Glass · uinput
                 UEFI → systemd-boot → Hyperswitch host
```

- **Input:** the daemon grabs your physical keyboard/mouse and forwards events only
  to the active OS. Each OS has its own permanent virtual device, so switching is
  just a pointer change.
- **Display:** GPU passthrough + Looking Glass for near-native speed, or SPICE on
  single-GPU machines.
- **Audio:** inactive OSes are muted, not stopped.

Full details: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Requirements

| | Minimum | Recommended |
|---|---|---|
| CPU | VT-x / AMD-V | + VT-d / AMD-Vi (IOMMU), 8+ cores |
| RAM | 16 GB | 32 GB |
| GPU | 1 (virtual GPU mode) | 2 (one passed through) |
| Disk | SSD | NVMe |

Check a machine:

```sh
cargo run -p hs-hwcheck
```

## Quick start (development, no KVM needed)

```sh
cargo test --workspace
cargo run -p hs-daemon -- --mock -c config/hyperswitch.toml -s /tmp/hs.sock &
cargo run -p hs-cli -- -s /tmp/hs.sock status
cargo run -p hs-cli -- -s /tmp/hs.sock next
#   ubuntu → win11 in 0.02 ms
```

## Real host

```sh
sudo apt install libvirt-dev                 # build dependency
cargo build --release -p hs-daemon --features host
sudo ./scripts/setup-vfio.sh 10de:2484 10de:228b --apply   # reserve the 2nd GPU
sudo virsh define vm-templates/win11.xml
sudo cp config/hyperswitch.toml /etc/hyperswitch/          # edit devices + OS list
sudo ./target/release/hyperswitchd
```

Build the bootable image: `cd distro && mkosi -f build`.

## CLI

```
hyperswitch status          list OSes and which one has focus
hyperswitch switch <id>     jump to an OS
hyperswitch next | prev     cycle
hyperswitch start <id>      boot an OS in the background
hyperswitch stop <id>       shut an OS down
```

Hotkeys (configurable): `Ctrl+Alt+→` / `←` to cycle, `Ctrl+Alt+1…9` to jump.

## Project layout

```
crates/
  hs-core/      config · backend · input · hooks · switcher · ipc  (library)
  hs-daemon/    hyperswitchd
  hs-cli/       hyperswitch
  hs-hwcheck/   hyperswitch-check
config/         example hyperswitch.toml
vm-templates/   libvirt domains (pinning, hugepages, VFIO, evdev input)
distro/         mkosi image, systemd units, udev rules, sway kiosk, audio hook
scripts/        setup-vfio.sh
docs/           ARCHITECTURE.md
```

Cargo features: `libvirt` (real hypervisor), `evdev` (real input), `host` (both).

## Roadmap

- [x] Switch engine, IPC, CLI, hardware check
- [ ] Tauri overlay switcher UI
- [ ] Shared clipboard and virtiofs folders
- [ ] Graphical installer
- [ ] Laptop / single-GPU splitting (SR-IOV, GVT-g)

## License

Apache-2.0
