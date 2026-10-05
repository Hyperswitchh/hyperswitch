//! Pre-install hardware check. Exit code 0 = ready, 1 = blocking issue.
//! The installer runs this first and refuses to continue on FAIL.

use std::{fs, path::Path};

// === Types

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
enum Level {
    Pass,
    Warn,
    Fail,
}

struct Check {
    name: &'static str,
    level: Level,
    detail: String,
}

// === Entry

fn main() {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();

    let checks = vec![
        cpu_virt(&cpuinfo),
        kvm_device(),
        iommu(&cmdline),
        ram(&meminfo),
        gpus(),
        uinput(),
    ];

    println!("Hyperswitch hardware check\n");
    for c in &checks {
        let tag = match c.level {
            Level::Pass => "PASS",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        };
        println!("  [{tag}] {:<22} {}", c.name, c.detail);
    }
    let worst = checks.iter().map(|c| c.level).max().unwrap_or(Level::Pass);
    println!();
    match worst {
        Level::Pass => println!("Ready: full-speed mode (GPU passthrough) supported."),
        Level::Warn => println!("Usable: Hyperswitch will fall back to virtual GPUs where needed."),
        Level::Fail => {
            println!("Not ready: fix the FAIL items (usually BIOS settings) and re-run.");
            std::process::exit(1);
        }
    }
}

// === Checks

fn cpu_virt(cpuinfo: &str) -> Check {
    let flags = cpuinfo
        .lines()
        .find(|l| l.starts_with("flags"))
        .unwrap_or("");
    let (level, detail) = if flags.contains(" vmx") {
        (Level::Pass, "Intel VT-x".to_string())
    } else if flags.contains(" svm") {
        (Level::Pass, "AMD-V".to_string())
    } else {
        (
            Level::Fail,
            "no VT-x/AMD-V, enable virtualization in BIOS/UEFI".to_string(),
        )
    };
    return Check {
        name: "CPU virtualization",
        level,
        detail,
    };
}

fn kvm_device() -> Check {
    let ok = Path::new("/dev/kvm").exists();
    return Check {
        name: "/dev/kvm",
        level: if ok { Level::Pass } else { Level::Fail },
        detail: if ok {
            "present".into()
        } else {
            "missing, load kvm_intel / kvm_amd".into()
        },
    };
}

fn iommu(cmdline: &str) -> Check {
    let groups = fs::read_dir("/sys/kernel/iommu_groups")
        .map(|d| d.count())
        .unwrap_or(0);
    let (level, detail) = if groups > 0 {
        (Level::Pass, format!("{groups} IOMMU groups"))
    } else if cmdline.contains("intel_iommu=on") || cmdline.contains("amd_iommu") {
        (
            Level::Warn,
            "kernel flag set but no groups, enable VT-d/AMD-Vi in BIOS".into(),
        )
    } else {
        (
            Level::Warn,
            "off, GPU passthrough unavailable (add intel_iommu=on / amd_iommu=on)".into(),
        )
    };
    return Check {
        name: "IOMMU",
        level,
        detail,
    };
}

/// The kernel reports less than the installed RAM (firmware and GPU reservations), so a
/// 16 GB machine shows about 14.8 GB. The FAIL line sits below that to avoid rejecting it.
fn ram_level(gb: f64) -> Level {
    return match gb {
        g if g >= 30.0 => Level::Pass,
        g if g >= 12.0 => Level::Warn,
        _ => Level::Fail,
    };
}

fn ram(meminfo: &str) -> Check {
    let kb: u64 = meminfo
        .lines()
        .find(|l| l.starts_with("MemTotal:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let gb = kb as f64 / 1024.0 / 1024.0;
    return Check {
        name: "RAM",
        level: ram_level(gb),
        detail: format!("{gb:.1} GB usable (16 GB installed minimum, 32 recommended)"),
    };
}

/// PCI class 0x03xxxx covers VGA, 3D and display controllers. The DRM card list is not
/// used because a GPU without an active driver or outputs has no `cardN` entry.
fn count_display_devices(classes: &[String]) -> usize {
    return classes
        .iter()
        .filter(|c| c.trim().starts_with("0x03"))
        .count();
}

fn gpus() -> Check {
    let classes: Vec<String> = fs::read_dir("/sys/bus/pci/devices")
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter_map(|e| fs::read_to_string(e.path().join("class")).ok())
                .collect()
        })
        .unwrap_or_default();
    let n = count_display_devices(&classes);
    let (level, detail) = match n {
        0 => (Level::Warn, "none detected".to_string()),
        1 => (
            Level::Warn,
            "1 GPU, guests share it via virtual GPU (slower graphics)".to_string(),
        ),
        n => (Level::Pass, format!("{n} GPUs, one can be passed through")),
    };
    return Check {
        name: "GPUs",
        level,
        detail,
    };
}

fn uinput() -> Check {
    let ok = Path::new("/dev/uinput").exists();
    return Check {
        name: "/dev/uinput",
        level: if ok { Level::Pass } else { Level::Fail },
        detail: if ok {
            "present".into()
        } else {
            "missing, run `modprobe uinput`".into()
        },
    };
}

// === Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_gb_machine_is_not_rejected() {
        assert_eq!(ram_level(14.8), Level::Warn);
        assert_eq!(ram_level(31.0), Level::Pass);
        assert_eq!(ram_level(8.0), Level::Fail);
    }

    #[test]
    fn counts_vga_3d_and_display_classes_only() {
        let classes: Vec<String> = [
            "0x030000\n",
            "0x030200\n",
            "0x038000\n",
            "0x060400\n",
            "0x020000\n",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(count_display_devices(&classes), 3);
    }
}
