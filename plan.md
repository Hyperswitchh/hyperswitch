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

## Phase 5: Windows guest on virtual graphics (done)

Setup:
- `vm-templates/win11-virtual.xml`: UEFI with Secure Boot, TPM 2.0 (swtpm), 4 GB RAM, 4 vCPUs, SPICE with QXL, SATA system disk, Windows ISO and virtio-win ISO as CD-ROMs, guest-agent channel, Hyperswitch evdev input. Validated against the libvirt schema.
- Windows 11 (build 26300) installed on a 64 GB sparse qcow2 under `qemu:///session`. First-run setup finished with the optional offers declined. The Windows ISO was ejected after the first restart so the guest cannot fall back into Setup.
- virtio-win 0.1.302 installed in the guest (drivers, QEMU guest agent, display and SPICE tools). The ISO download was truncated twice because my `curl ... | tail` hid curl's exit code. It was re-fetched with retries and verified by exact size.

Findings:
- Switching the system disk to virtio right after installing the drivers sent Windows into automatic repair (the storage driver was not yet a boot driver). Rolled back to SATA, which boots. The network is virtio. Needs `sc config viostor start= boot` or a spare virtio disk first. Not blocking.
- A full Windows install plus first boot took about 20 GB of the host disk. Free space fell from 52 GB to about 24 GB.
- `virsh shutdown` did not shut Windows down while it was doing first-boot background work (4 vCPUs busy). It worked once the guest was idle.
- Startup race reproduced: `hyperswitchd` creates the virtual devices and starts guests about 30 ms later, before udev has made the `/dev/input/hyperswitch/...` symlink, so QEMU fails to open it and the daemon exits. Workaround used: `autostart = false` and start guests after the symlinks exist. Fix to build: wait for the symlinks before `boot_all`.
- uaccess ACLs on a new input device can arrive after the daemon opens it ("permission denied"). Wait until the node is readable.
- BUG FOUND AND FIXED: when a combo fired, the router dropped the rest of that input batch, including the combo key's release. The matcher kept the key "held" and ignored its next press as auto-repeat, so every second hotkey failed (22 of 42 switched) and the keys leaked to the guest as plain arrow keys. Fix: `HotkeyMatcher::on_batch` applies every event in the batch. A unit test and a hardware-test case (combo key press and release in one write) reproduce it, and fail on the old code.

Gate (Alpine test guest and Windows 11, fixed daemon, fake keyboard, real libvirt and QEMU):
- 20 round trips, each typing in the focused guest and switching with Ctrl+Alt+Right and Ctrl+Alt+Left: 42 of 42 switches registered, exactly as expected.
- Alpine received exactly 20 `a` characters and none of the digits, with no stray arrow keys.
- Windows' sign-in screen showed exactly 3 dots for 3 digits typed in an earlier run, and an earlier hotkey woke it from a black screen. In the final run Windows was already signed in, so its characters were not counted; routing is inferred from Alpine receiving none of them.
- Switch time per hotkey, no display or audio hooks: min 219 us, median 1.1 ms, p95 1.4 ms, max 1.4 ms.

Not done: the virtio system disk, auditing Windows' key count in the final run, and real display and audio hooks (Phase 6).

## Phase 6: Measure (done)

Work done:
- The switch engine now times each step (`StepTimes`: state check, input, blur hook, focus hook). The numbers are in `SwitchReport`, the CLI output and the daemon log. Tests added.
- `scripts/bench-switch.py` sends N `next` requests over the daemon socket and prints median, p95 and max per step.

Setup: `hyperswitchd` against the real libvirt guests (Alpine and Windows 11 both running under `qemu:///session`), input devices empty so no input router, 200 switches per scenario. Machine under normal load (Windows guest using about 0.7 of a core plus the desktop, load average 6 to 13 on 12 threads), so these are not best-case numbers.

| Scenario | Median | p95 | Max |
|---|---|---|---|
| S0 no hooks | 0.14 ms | 0.18 ms | 0.40 ms |
| S1 two bare shell spawns (`true`) | 1.16 ms | 1.41 ms | 1.56 ms |
| S2 real `audio.sh` on real PipeWire | 20.8 ms | 23.0 ms | 25.8 ms |
| S3 `audio.sh` plus a display stand-in (`gdbus call`) | 25.1 ms | 28.8 ms | 41.2 ms |

Per step in S2: state check 0.2 ms, input 0.00 ms, blur hook 10.2 ms, focus hook 10.3 ms. No switch went over the 100 ms budget.

Where the time goes: libvirt's `state` call is about 0.15 ms and a shell spawn about 0.5 ms. Each `wpctl` process costs 8 to 9 ms by itself (PipeWire connection setup): `wpctl status` 9.5 ms, `wpctl set-mute <id>` 8.4 ms. `audio.sh` runs about two of them per hook, which is the whole cost.

What is real and what is not:
- Real: libvirt calls, guest state, shell hook spawn, `audio.sh` and `wpctl` against real PipeWire (two null sinks named `hs-test1` and `hs-win11`, created with `pw-cli`, since the guests have no audio devices).
- Stand-in: the display hook. Sway is not installed and this laptop runs GNOME, so a `gdbus` call to the session bus stood in for `swaymsg`. A real `swaymsg` round trip should be similar, but it is unmeasured.
- Not measured: the time from a key press to the guest's screen changing (needs frame timing from Looking Glass or SPICE), and input latency through the evdev path.

Result: under 2 s (pass) and under 100 ms (stretch goal met for the hook-driven part of a switch on this machine).

Options if a lower number is ever needed (not done, not needed now):
- Cache the PipeWire node id per guest so `audio.sh` skips the `wpctl status` lookup. Saves about half the audio cost.
- A native PipeWire client inside the daemon removes the per-switch process spawn and should bring the audio hooks under 1 ms.
- Run the two hooks in parallel.

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
0. On-demand mode and idle guest policy (details below). Do this first, because it also fixes the startup race that breaks every cold start.
1. Tray or overlay UI over the existing socket (Tauri). USB moves and snapshots are awkward to use without it.
2. Shared udev monitor in the daemon, built once for both USB plug events and input hotplug.
3. USB device passthrough (details below). Manual `usb attach`, `usb detach` and `usb move` ship first and do not need the monitor. Follow-focus and auto-attach need it.
4. Shared clipboard (SPICE vdagent) and shared folder (virtiofs).
5. Input hotplug through the shared udev monitor.
6. Snapshots, starting with a spike (details below).
7. Hardware compatibility list based on hwcheck results.
8. Bootable image and installer. This is last and does not target laptops in v1.

### On-demand mode and idle guest policy

Why: the user wants only one OS running when the machine starts, and to start the other only when needed, because two running guests slow the machine. Measured on this 14 GB laptop: free memory fell from about 7 GB to about 3 GB with Windows (4 GB) and Alpine running, and an idle Windows guest used 0.2 to 0.7 of a core, spiking to all 4 vCPUs during first-boot updates. RAM is the binding limit.

Already possible: `autostart = false` per OS, `hyperswitch start <id>` and `stop <id>`, and a switch to a stopped guest cold-starts it. The trade-off to state in the docs: switching is instant only while both guests are running. A cold-started Windows takes 30 to 60 seconds to boot.

Steps, in order:
1. **Fix the startup race.** After the router creates its virtual devices, wait (up to about 5 s) for the udev symlinks `/dev/input/hyperswitch/hyperswitch-<id>-<n>` before starting any guest, and wait until the physical device node is readable (udev ACLs can arrive after the node appears). Log a warning and carry on if the wait times out. This applies to `boot_all`, `start` and a cold start on switch. Seen twice: QEMU failed to open a symlink that did not exist yet.
2. **Make autostart off by default.** It currently defaults to on. The example config enables it only for the OS the machine boots into.
3. **`hyperswitch up <os>` and `hyperswitch down <os>`.** `up` starts the daemon if needed, starts the guest, waits until it is running and moves focus to it. `down` shuts the guest down, waits until it is off (with a timeout and a force option) and frees its RAM. The daemon can run as a user service started on demand.
4. **Idle guest policy:** `idle_guest = "run" | "pause" | "save"` in `hyperswitch.toml`, default `run`.
   - `pause` freezes the guest that lost focus and resumes it on switch back. It uses the existing `Hypervisor::pause`, which is currently unused. Frees CPU, not RAM. Guest clocks jump on resume.
   - `save` writes the guest to disk and frees its RAM. Spike first: test libvirt managed save on a Windows guest with a TPM, measure the restore time, refuse when passthrough devices are attached, and check free disk space first (up to the guest's RAM size per save).
5. The UI should show a "starting" state for a cold-started guest.

Notes:
- A guest that has the Hyperswitch evdev input attached cannot start unless the daemon is already running. `up` handles that order. Starting such a guest directly with virt-manager fails.
- With the future bootable image the host has no desktop OS, so "one OS at boot" means autostart for the first guest only.

Gate: on a fresh login only one OS is running and free RAM is not reduced by the other guest. `hyperswitch up win11` brings Windows up and switches to it. `hyperswitch down win11` returns the RAM. With `idle_guest = "pause"` the unfocused guest uses near zero CPU and the switch back takes well under a second.

### USB device passthrough

Give a USB device (flash drive, phone, dongle) to one OS at a time.

Principle: **USB devices never move as a side effect of switching OS.** Switching focus moves keyboard, mouse, screen and audio only. A device moves only when the user says so, which prevents corruption from a drive changing owner mid-write.

Three ways to move a device, all explicit:
1. **Button or command.** The overlay shows each USB device with a button such as "Send to Windows". The CLI equivalent is `hyperswitch usb move <device> <os>`. Before the move the daemon checks that the current owner is not using the device and asks the user to confirm a safe eject. See the safety rules for what the daemon can and cannot do for a guest.
2. **Unplug and replug (follow focus).** The user unplugs the device, switches to the OS they want and plugs it back in. The daemon gives the new device to whichever OS is active at that moment. If the active OS is the host, the daemon does nothing and the host keeps the device. Controlled by `usb_follow_focus = true | false` in `hyperswitch.toml`, so some users can keep every new device on one OS.
3. **Auto-attach rules** in `hyperswitch.toml` (for example, a phone always goes to win11).
Precedence: an explicit rule wins over follow focus, which wins over leaving the device on the host.

Implementation:
- libvirt `<hostdev mode='subsystem' type='usb'>` matched by vendor and product id, attached live (`virsh attach-device --live`, or the `virt` crate equivalent).
- IPC and CLI commands: `usb list`, `usb attach <guest> <device>`, `usb detach <guest> <device>`, `usb move <device> <os>`.
- Verify, do not assume: QEMU may reconnect a replugged device to the guest it was attached to before, matching by vendor and product id. If it does, the daemon must detach the device on unplug so the replug decision is made fresh from the active OS.

Safety rules (enforced by the daemon, with a clear error):
- Refuse any device listed in `input_devices`. The daemon owns those.
- Never auto-pass keyboards and mice (USB HID class). A newly plugged keyboard or mouse goes to the input router so it follows the hotkey. This needs input hotplug.
- Refuse hubs (USB class 09) and the device that holds the host's root disk.
- Warn before passing an internal device. This laptop has a Broadcom 58200 (likely fingerprint or smart card reader), a webcam and an AX201 Bluetooth controller on the USB bus. Passing one takes it away from the host.
- Host side: refuse while a partition of the device is mounted on the host, or sync and unmount it first. Attaching it while the host has it mounted is a surprise removal and can lose unwritten data.
- Guest side: the daemon cannot unmount inside a guest without extra tooling (the qemu-guest-agent has no unmount command, so this would need `guest-exec` and per-OS scripts). Version one asks the user to confirm they ejected it in the guest, and says so in the prompt. Moving a drive without a safe eject can corrupt NTFS or exFAT.
- Physically unplugging is the user's choice and is a surprise removal. It is safe only when nothing is copying. The button path is the recommended one and the CLI output says so.
- A device is exclusive to one OS while attached. Say so in the CLI output.

Plug and unplug events (need the shared udev monitor):
- Watch for USB devices appearing and disappearing.
- Unplug while attached: mark the device as detached, drop it from the daemon's state and log it.
- Replug: apply the rules above (explicit rule, then follow focus, then stay on the host).
- Gate: unplug and replug a flash drive that is attached to a guest. The daemon state stays correct and the drive lands on the active OS.

Notes:
- Under `qemu:///session` the guest cannot open USB device nodes without a udev rule. Test under `qemu:///system`.
- A guest with a passed-through device generally cannot take a memory snapshot. Detach first.

Gate: a flash drive moves between two running guests in both directions using the button or `usb move`, with a safe eject each time and without a reboot, and the files on it are readable in each. A second run proves unplug and replug follows focus, and that switching OS alone never moves the drive.

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
| Device unplugged while attached to a guest | Stale state, guest sees a dead device | Udev monitor updates state, replug follows the active OS |
| Guest cannot be told to unmount a drive | Corruption when moving a drive | Ask the user to confirm a safe eject, never move it as a side effect of switching |

## Success criteria

- Minimum: two guests running at once on this laptop, switched by hotkey with correct input, screen and audio, in under 2 seconds.
- Target: under 100 ms with no stuck keys over 1000 switches.
- Stretch: Windows with the P620 through Looking Glass.

## Decisions pending

- Where Ubuntu lives: stay as the host (recommended) or move into a guest later.
- Which second guest to use for early testing: Alpine or Ubuntu Server.
- Whether to install the virtualization packages now (Phase 2).
