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
| 3 | Hotkey batch leaked to the old guest and key-ups reached the new one | Fixed: the batch that completes the combo is dropped. Mouse buttons are now released too. Verified by the Phase 4 hardware test |
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

## Phase 2: Install virtualization on this laptop (done)

- Installed (70 packages, 60 MB): libvirt 12.0.0, virt-manager 5.1.0, swtpm 0.10.1, libvirt-dev, virtiofsd. QEMU 10.2.1 and OVMF were already present.
- `libvirtd`, its socket and `virtlogd` are active. The `default` NAT network is active and set to autostart.
- User added to the `kvm` and `libvirt` groups. The current login session does not have them until the next login. Until then run commands as `sg libvirt -c "<command>"`.
- Gate passed: `virsh -c qemu:///system list --all` runs and lists no guests. `/dev/kvm` is readable and writable with the kvm group.
- Disk: still 52 GB free.
- CI already built the `libvirt` and `evdev` features against real libvirt, so the `virt` 0.4 calls compile as written. Runtime behavior is still untested.
- Watch for: Docker and libvirt both edit firewall rules, so guest networking can conflict.

## Phase 3: Real backend with a light guest (done)

Setup: two Alpine 3.24.2 live guests (`hs-test1`, `hs-test2`, 512 MB each, SPICE, no disk) under `qemu:///session`. ISO in `~/hyperswitch-vms/`, sha256 verified. Daemon built with `--features hs-daemon/libvirt` and run with `--libvirt-uri qemu:///session`. `qemu:///system` is not tested yet.

| Check | Result |
|---|---|
| `status`, `next`, `prev`, `switch`, `start`, `stop` | All work against real libvirt |
| Switch between two running guests | 0.2 to 1.3 ms (no display or audio hooks yet) |
| Paused target (`virsh suspend`) | Reported as Paused, resumed on switch |
| Destroyed target | Reported as ShutOff, cold started on switch in 125 ms, flagged as cold boot |
| Unknown id | Clear error |
| SIGTERM | Clean exit, guests keep running |

Findings:
- `start` on a running guest returns a raw libvirt error. Making it idempotent would be friendlier.
- libvirt prints its own error line to stderr (`libvirt: QEMU Driver error`). Registering a silent error callback would remove the noise.
- `stop` sent ACPI shutdown but the Alpine live ISO has no `acpid`, so it kept running. That is guest behavior, not a daemon bug.
- Item 7 (mutex held across slow calls) matters less than expected: libvirt `shutdown` returns immediately. Revisit only if a measured stall appears.

Gate passed. The two guests are left running for Phase 4.

## Phase 4: Real input routing (router verified, QEMU wiring and real keyboard pending)

Safety setup: a test-only udev rule (`/etc/udev/rules.d/71-hyperswitch-test.rules`) gives the user access to `/dev/uinput` and to devices named `hs-fake-*` or `hyperswitch-*`, and sets `LIBINPUT_IGNORE_DEVICE` so the GNOME desktop never types them. Remove it when Phase 4 is finished: `sudo rm /etc/udev/rules.d/71-hyperswitch-test.rules && sudo udevadm control --reload`.

Hardware test: `crates/hs-core/tests/uinput_router.rs` (ignored by default).
Run: `cargo test -p hs-core --features evdev --test uinput_router -- --ignored`

What it does: fake keyboard via uinput, real `EvdevRouter` with two virtual guest devices, 100 switch cycles. Each cycle checks that a plain key reaches only the active guest, that the hotkey fires, and that the physical releases afterward do not reach the new guest. At the end no key may be stuck down.

Results:
- Passes with the fixes (100 cycles, about 6 s).
- Fails when the original behavior is restored: Ctrl, Alt and Right stay down on the old guest and the releases leak to the new one. So the test does detect the bug.
- Correction to the Phase 1 notes: the Linux input layer already drops a key-up (and repeat) for a key that is not down on a virtual device, so an earlier `KeySuppressor` was redundant. It was removed and the test still passes. The fix that matters is dropping the batch that completes the combo.
- Finding: the distro rule `70-hyperswitch.rules` should also set `LIBINPUT_IGNORE_DEVICE` for `hyperswitch-*` devices.

Still to do for this phase:
- Wire the guests to the virtual devices with libvirt `<input type="evdev">`, using the udev symlinks from the distro rule. The daemon already creates the devices before it starts guests.
- Check typing reaches only the focused guest with a real guest, then repeat with the real keyboard behind a watchdog.

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
