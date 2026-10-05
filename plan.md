# Hyperswitch Plan

Goal: run Ubuntu and Windows at the same time as KVM guests and move keyboard, mouse, screen and audio between them with one hotkey, in seconds (stretch target: under 100 ms).

This laptop is the test machine. The plan is shaped around what it can do.

## Test machine

| Item | Value |
|---|---|
| Model | Dell Precision 3551 (laptop) |
| CPU | Intel i7-10850H, 12 threads, VT-x on |
| RAM | 14 GB (about 7 GB free in normal use) |
| GPUs | Intel UHD (drives the only screen) + NVIDIA Quadro P620, 2 GB, no display outputs |
| IOMMU | Active, 21 groups. P620 shares a group only with its PCIe root port |
| Disk | One 238 GB NVMe, EFI + Ubuntu ext4. No Windows installed |
| Installed | qemu-system-x86_64, cargo. Not installed: libvirt, virt-manager, Looking Glass |
| Access | `/dev/kvm` needs the `kvm` group. `/dev/uinput` is root only |

What this means:
- Both guests plus the host must fit in 14 GB. Budget: host 4 GB, Windows 6 GB, second guest 3 GB.
- Switching with virtual graphics (SPICE or virtio-gpu) is realistic here.
- Passing the P620 to Windows is an experiment, not a milestone. It has no outputs, which suits Looking Glass, but laptop firmware and NVIDIA drivers can block it.
- All guest disks are qcow2 files. Do not repartition the disk and do not boot any bare-metal partition as a VM.

## Rules for this project
- Follow `~/.claude/skills/coding-assistant.md`: explicit `return`, named type in constructors, `// ===` section separators, no em dashes, single-line conventional commits.
- No file changes without an approved plan for that step.
- Each phase has a test gate. Do not start the next phase until the gate passes.

## Phase 0: Baseline (done)

- fmt, clippy and tests pass. Mock smoke test passes. Git repo initialised, duplicate copies removed.
- Layout: the project lives at `Desktop/Hyperswitch/docs/hyperswitch/`.
- `hs-hwcheck` baseline saved to `docs/hwcheck-baseline.txt`.
- Disk: 52 GB free of 233 GB, below the 80 GB wanted in Phase 2. Two small guests will fit.

## Phase 1: Fix known bugs (software only) (done except items 4 and 7)

| # | Bug | Status |
|---|---|---|
| 1 | `boot_all` focus hook failure killed the daemon at startup | Fixed: logged as a warning. Test added |
| 2 | Hook failure left input and screen on different guests | Fixed: input is moved back and focus restored. Test added |
| 3 | Hotkey batch leaked to the old guest and key-ups reached the new one | Fixed: combo batch dropped, `KeySuppressor` drops matching key-ups and repeats. Tests added. Mouse buttons are now released too |
| 4 | Switching to a stopped VM returns before it has booted | Reworked, no code change: libvirt reports `Running` as soon as QEMU starts, so the guest OS boot cannot be detected from the hypervisor. A switch to a cold guest still moves focus so the user sees it boot, and the report flags `cold_start` |
| 5 | `DisplayMode` was unused and hooks hardcoded `lg-{id}` | Fixed: `{window}` and `{monitor_input}` placeholders come from `DisplayMode`. SPICE guests now match by title. Tests added |
| 6 | `mirror()` copied only keys and relative axes | Fixed: absolute axes are copied. LEDs are not supported by the uinput builder in evdev 0.12, so they stay unmirrored |
| 7 | Libvirt connection mutex held across slow calls | Deferred to Phase 3: needs libvirt headers to compile and test |
| 8 | Only `ctrl_c` was handled | Fixed: SIGTERM handled, socket removed. Verified. Too-long socket paths now give a clear error |
| 9 | Example config pointed at nonexistent devices | Fixed: `input_devices = []` with instructions |
| 10 | Style: implicit returns, `Self {}`, separators, em dashes | Fixed. `clippy::needless_return` is allowed workspace-wide in `Cargo.toml` because the style rule requires explicit returns |
| 11 | hwcheck failed any 16 GB machine (kernel reports about 14.8 GB) | Fixed: FAIL below 12 GB, WARN up to 30 GB. Test added |
| 12 | hwcheck counted DRM cards, so it saw 1 GPU on a 2 GPU laptop | Fixed: counts PCI display class devices. Test added |

Result: 18 tests pass (8 more than baseline), clippy clean with and without the `evdev` feature.

Not verified: the `libvirt` feature (needs `libvirt-dev`) and the real evdev router on a device. Both are Phase 3 and 4.

## Phase 2: Install virtualization on this laptop

Needs approval before any `apt install` or group change.

Work:
- Check free space: `df -h /`. Plan for at least 80 GB free for two guests.
- Install `libvirt-daemon-system libvirt-clients virt-manager ovmf swtpm libvirt-dev`.
- Add the user to `kvm` and `libvirt` groups, then log out and in.
- Confirm `virsh list --all` works.

Gate: `virsh -c qemu:///system list --all` runs without error.

## Phase 3: Real backend with a light guest

Use a small Linux guest first. It boots in seconds and uses 2 GB, so it tests the daemon without Windows complexity.

Work:
- Create guest `hs-test1` (Alpine or Ubuntu Server, 2 GB RAM, virtio, SPICE).
- Build `cargo build -p hs-daemon --features libvirt`. Fix any `virt` 0.4 API errors.
- Run `hyperswitchd` with a test config pointing at `hs-test1` and `hs-test2`.
- Exercise `status`, `start`, `stop`, `switch`, `next`.

Gate: all six CLI commands work against real libvirt guests. Pause and resume states report correctly.

## Phase 4: Real input routing

Work:
- Build with `--features host`. Run the daemon as root, or grant `uinput` and `input` group access.
- Point `input_devices` at this laptop's keyboard and mouse under `/dev/input/by-id/`. Keep a second terminal or SSH session open in case input is lost.
- Check that udev creates `/dev/input/hyperswitch/...` and that QEMU `<input type="evdev">` accepts them.
- Test the hotkeys: next, prev, direct jump.
- Check for stuck modifiers and leaked key events (Phase 1 item 3).

Safety: test first with a throwaway virtual device on `/dev/uinput`, not the real keyboard. Add a watchdog that releases the grab after N seconds during testing.

Gate: typing goes only to the focused guest. No stuck keys over 100 switches.

## Phase 5: Windows guest on virtual graphics

Work:
- Fetch a Windows 11 ISO and the virtio-win drivers ISO. Needs approval for each download.
- Create `hs-win11` with a qcow2 disk, OVMF, swtpm and virtio, using a trimmed copy of `vm-templates/win11.xml` without the `hostdev` and `shmem` sections.
- Install Windows. Install virtio drivers and the SPICE guest tools.
- Add to the config as `display = spice`. Run all of Phase 3 and 4 tests with Windows.

Gate: switch between Ubuntu host window and the Windows guest by hotkey, in both directions, repeatedly.

## Phase 6: Measure

Work:
- Add timing around each step of `switch_index` (state check, input, blur, focus).
- Run 200 switches. Record median, p95 and max.
- Replace slow hooks (`sh`, `swaymsg`, `wpctl status | awk`) if they exceed the budget.

Gate: results are written down. A switch that completes in under 2 seconds counts as a pass for this machine. Under 100 ms is the stretch goal.

## Phase 7: Optional GPU passthrough experiment

Risky, so only after Phase 6 and with a rollback ready.

Work:
- Confirm the P620 and its audio function are alone in the IOMMU group. Run `scripts/setup-vfio.sh` in dry-run mode first.
- Back up `/etc/modprobe.d` and the initramfs. Know how to boot without vfio (GRUB edit).
- Bind the P620 to `vfio-pci`, add it to the Windows guest, install the NVIDIA driver, add Looking Glass.
- Expect problems: NVIDIA Code 43, missing ACPI battery or vbios, host losing NVIDIA compute.

Gate: either Windows shows the P620 in Device Manager and Looking Glass displays frames, or the result is documented as not supported on this laptop and we stay on virtual graphics.

## Phase 8: Product features

Order, each needing its own plan:
1. Tray or overlay UI over the existing socket (Tauri).
2. Shared clipboard (SPICE vdagent) and shared folder (virtiofs).
3. Input hotplug through a udev monitor.
4. Hardware compatibility list based on hwcheck results.
5. Bootable image and installer. This is last and does not target laptops in v1.

## Risks

| Risk | Effect | Mitigation |
|---|---|---|
| Only 14 GB RAM | Guests swap or get killed | Strict RAM budget, close heavy apps during tests |
| Input grab goes wrong | Laptop keyboard becomes unusable | Test on a virtual device first, watchdog, SSH session |
| P620 passthrough fails | No fast graphics | Treat as optional, keep SPICE path |
| Disk space | Cannot store two guests | Check `df` in Phase 2, use sparse qcow2 |
| Windows licence and ISO download | Delay | User supplies licence, approve each download |
| `virt` or `evdev` crate API mismatch | Build errors | Fix in Phase 3 against the real crates |
| Hybrid graphics changes with driver updates | Passthrough breaks | Pin the driver version during the experiment |

## Success criteria

- Minimum: two guests running at once on this laptop, switched by hotkey with correct input, screen and audio, in under 2 seconds.
- Target: under 100 ms with no stuck keys over 1000 switches.
- Stretch: Windows with the P620 through Looking Glass.

## Decisions pending

- Where Ubuntu lives: stay as the host (recommended) or move into a guest later.
- Which second guest to use for early testing: Alpine or Ubuntu Server.
- Whether to install the virtualization packages now (Phase 2).
