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

## Phase 4: Real input routing (done, real keyboard step skipped by choice)

Safety setup: a test-only udev rule (`/etc/udev/rules.d/71-hyperswitch-test.rules`) gives the user access to `/dev/uinput` and to devices named `hs-fake-*` or `hyperswitch-*`, and sets `LIBINPUT_IGNORE_DEVICE` so the GNOME desktop never types them. Remove it when Phase 4 is finished: `sudo rm /etc/udev/rules.d/71-hyperswitch-test.rules && sudo udevadm control --reload`.

Hardware test: `crates/hs-core/tests/uinput_router.rs` (ignored by default).
Run: `cargo test -p hs-core --features evdev --test uinput_router -- --ignored`

What it does: fake keyboard via uinput, real `EvdevRouter` with two virtual guest devices, 100 switch cycles. Each cycle checks that a plain key reaches only the active guest, that the hotkey fires, and that the physical releases afterward do not reach the new guest. At the end no key may be stuck down.

Results:
- Passes with the fixes (100 cycles, about 6 s).
- Fails when the original behavior is restored: Ctrl, Alt and Right stay down on the old guest and the releases leak to the new one. So the test does detect the bug.
- Correction to the Phase 1 notes: the Linux input layer already drops a key-up (and repeat) for a key that is not down on a virtual device, so an earlier `KeySuppressor` was redundant. It was removed and the test still passes. The fix that matters is dropping the batch that completes the combo.
- Finding: the distro rule `70-hyperswitch.rules` should also set `LIBINPUT_IGNORE_DEVICE` for `hyperswitch-*` devices.

Guest wiring (done, with two Alpine guests under `qemu:///session`):
- Each guest has `<input type='evdev'>` pointing at `/dev/input/hyperswitch/hyperswitch-<id>-0` with `grab='all'`, added with `virsh attach-device --config`. QEMU opened the devices (`input-linux` with `grab_all` and `repeat`).
- The daemon created the virtual devices and the udev symlinks first, then booted the guests. Both started without errors.
- A fake keyboard (scratch tool, not in the repo) typed into the system. Guest screens were read with `virsh screenshot` while `cat -v` ran in each guest.
- Result: `aaa` reached only the focused guest 1. Ctrl+Alt+Right switched in 1.36 ms and `bbb` reached only guest 2. Ctrl+Alt+Left switched back in 1.34 ms and `ccc` reached guest 1. No `^` characters appeared, so no modifier stuck. Display and audio hooks were empty in this run.

Findings:
- `virt-install` ejects the install ISO after the first boot, so a restarted guest says "No bootable device". Re-insert it with `virsh change-media <guest> hda <iso> --insert --config --live`.
- Guests with an `evdev` input cannot start unless the daemon has already created the symlinks. The daemon creates the devices before it starts guests and it worked here, but it does not wait for udev. A short wait for the symlinks before `boot_all` would remove a possible race.
- `virsh send-key` presses keys together, so doubled letters are dropped. Send keys one at a time.
- Typing latency was not measured.

Skipped by choice: the real keyboard test. The test udev rule `/etc/udev/rules.d/71-hyperswitch-test.rules` should be removed when no longer needed.

## Phase 5: Windows guest on virtual graphics (in progress)

Done so far:
- `vm-templates/win11-virtual.xml` added: UEFI with Secure Boot, TPM 2.0 (swtpm), 4 GB RAM, 4 vCPUs, SPICE with QXL, SATA system disk and e1000e network (both inbox Windows drivers, so the installer needs no driver step), Windows ISO and virtio-win ISO as CD-ROMs, and the Hyperswitch evdev input. Placeholders `@DISK@`, `@WIN_ISO@`, `@VIRTIO_ISO@`. It validates against the libvirt schema.
- `vm-templates/win11.xml` evdev inputs now use the form verified in Phase 4 (`grab="all"`, `repeat="on"`).
- virtio-win 0.1.302 ISO (877 MB, fedorapeople.org) downloading to `~/hyperswitch-vms/`.

Waiting on: a Windows 11 x64 ISO supplied by the user (Microsoft's download link is per session, so it cannot be scripted).

Next: create a 64 GB sparse qcow2 (disk has about 51 GB free, Windows will use roughly 25 GB), define the guest under `qemu:///session`, install Windows, install the virtio drivers and SPICE guest tools, then run the Phase 3 and 4 checks with Windows as one of the two guests.

Gate: switch between the Ubuntu host window and the Windows guest by hotkey, in both directions, repeatedly.

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

Start only after Phase 5 and 6 are done. Everything below depends on a working Windows guest and a measured switch time. Each item needs its own plan before any code.

Order:
1. Tray or overlay UI over the existing socket (Tauri). Both items below are awkward to use without it.
2. USB device passthrough (details below).
3. Shared clipboard (SPICE vdagent) and shared folder (virtiofs).
4. Input hotplug through a udev monitor.
5. Snapshots, starting with a spike (details below).
6. Hardware compatibility list based on hwcheck results.
7. Bootable image and installer. This is last and does not target laptops in v1.

### USB device passthrough

Give a USB device (flash drive, phone, dongle) to one guest at a time.
- Use libvirt `<hostdev mode='subsystem' type='usb'>` matched by vendor and product id, attached live (`virsh attach-device --live`, or the `virt` crate equivalent).
- Add IPC and CLI commands: `usb list`, `usb attach <guest> <device>`, `usb detach <guest> <device>`.
- Optional: auto-attach rules in `hyperswitch.toml` (for example, a phone always goes to win11).

Safety rules (all enforced by the daemon, with a clear error):
- Refuse any device listed in `input_devices`. The daemon owns those.
- Refuse hubs (USB class 09) and the device that holds the host's root disk.
- Warn before passing an internal device. This laptop has a Broadcom 58200 (likely fingerprint or smart card reader), a webcam and an AX201 Bluetooth controller on the USB bus. Passing one takes it away from the host.
- Refuse while the host has a partition of the device mounted, or sync and unmount it first. Attaching it with the host mounting it is a surprise removal and can lose unwritten data.
- Moving a drive between guests requires a safe eject in the first guest. Detaching without it can corrupt NTFS or exFAT.
- A device is exclusive to one guest while attached. Say so in the CLI output.

Notes:
- Under `qemu:///session` the guest cannot open USB device nodes without a udev rule. Test under `qemu:///system`.
- A guest with a passed-through device generally cannot take a memory snapshot. Detach first.

Gate: a flash drive moves between two running guests in both directions, with a safe eject each time and without a reboot, and the files on it are readable in each.

### Snapshots

Do a spike before any feature work.
- Spike: on `hs-win11`, take a disk-only external snapshot by hand with `virsh`, change a file, revert, and confirm the file is back. Libvirt 12.0.0 `snapshot-revert` has a `--reset-nvram` option, so UEFI NVRAM is handled, but reverting external snapshots is unproven here.
- If revert works: add thin IPC and CLI commands over it (`snapshot create`, `list`, `revert`, `delete <guest> [name]`). Check free disk space first and refuse if too low. Memory snapshots can write up to 6 GB for Windows, and this laptop has about 50 GB free.
- If revert does not work: drop the item. virt-manager and `virsh` already do snapshots, and Hyperswitch's value is switching.
- Limits: guests with passed-through devices (USB now, GPU later) cannot take memory snapshots. The overlay UI can show the snapshot list later.

Gate: snapshot each guest, change a file, revert, confirm the file is back to its old state, and confirm the guest is still switchable.

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
| USB device passed while mounted on the host | Data loss | Daemon refuses until unmounted, safe eject before moving |
| Passing an internal USB device (Bluetooth, webcam) | Host loses it | Warn before attaching, require exact vendor and product id |
| External snapshot revert not supported | Snapshot feature unusable | Spike first, drop the item if it fails |

## Success criteria

- Minimum: two guests running at once on this laptop, switched by hotkey with correct input, screen and audio, in under 2 seconds.
- Target: under 100 ms with no stuck keys over 1000 switches.
- Stretch: Windows with the P620 through Looking Glass.

## Decisions pending

- Where Ubuntu lives: stay as the host (recommended) or move into a guest later.
- Which second guest to use for early testing: Alpine or Ubuntu Server.
- Whether to install the virtualization packages now (Phase 2).
