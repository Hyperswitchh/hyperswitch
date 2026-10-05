//! Hypervisor abstraction.
//!
//! Production uses [`libvirt::LibvirtHypervisor`] (feature `libvirt`), which talks to
//! KVM/QEMU through `qemu:///system`. Tests and dev machines without KVM use
//! [`MockHypervisor`].

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Mutex};
use thiserror::Error;

// === Types

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmState {
    Running,
    Paused,
    ShutOff,
    Crashed,
    Unknown,
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("domain `{0}` not found")]
    NotFound(String),
    #[error("hypervisor error: {0}")]
    Other(String),
}

pub type BResult<T> = Result<T, BackendError>;

/// Everything the switch engine needs from the hypervisor.
///
/// Calls are blocking; the daemon wraps them in `spawn_blocking`.
pub trait Hypervisor: Send + Sync {
    fn state(&self, domain: &str) -> BResult<VmState>;
    fn start(&self, domain: &str) -> BResult<()>;
    fn shutdown(&self, domain: &str) -> BResult<()>;
    fn pause(&self, domain: &str) -> BResult<()>;
    fn resume(&self, domain: &str) -> BResult<()>;
}

// === Mock

/// In-memory hypervisor for tests and `hyperswitchd --mock`.
#[derive(Default)]
pub struct MockHypervisor {
    vms: Mutex<HashMap<String, VmState>>,
}

impl MockHypervisor {
    pub fn with_domains<'a>(domains: impl IntoIterator<Item = &'a str>) -> Self {
        let vms = domains
            .into_iter()
            .map(|d| (d.to_string(), VmState::ShutOff))
            .collect();
        return MockHypervisor {
            vms: Mutex::new(vms),
        };
    }

    fn set(&self, domain: &str, f: impl FnOnce(VmState) -> VmState) -> BResult<()> {
        let mut vms = self.vms.lock().unwrap();
        let s = vms
            .get_mut(domain)
            .ok_or_else(|| BackendError::NotFound(domain.into()))?;
        *s = f(*s);
        return Ok(());
    }
}

impl Hypervisor for MockHypervisor {
    fn state(&self, domain: &str) -> BResult<VmState> {
        return self
            .vms
            .lock()
            .unwrap()
            .get(domain)
            .copied()
            .ok_or_else(|| BackendError::NotFound(domain.into()));
    }
    fn start(&self, d: &str) -> BResult<()> {
        return self.set(d, |_| VmState::Running);
    }
    fn shutdown(&self, d: &str) -> BResult<()> {
        return self.set(d, |_| VmState::ShutOff);
    }
    fn pause(&self, d: &str) -> BResult<()> {
        return self.set(d, |s| {
            if s == VmState::Running {
                VmState::Paused
            } else {
                s
            }
        });
    }
    fn resume(&self, d: &str) -> BResult<()> {
        return self.set(d, |s| {
            if s == VmState::Paused {
                VmState::Running
            } else {
                s
            }
        });
    }
}

// === Libvirt

#[cfg(feature = "libvirt")]
pub mod libvirt {
    //! Real backend over libvirt (`virt` crate). Build with `--features libvirt`.
    use super::*;
    use virt::{connect::Connect, domain::Domain, sys};

    pub struct LibvirtHypervisor {
        // Connect is not Sync; serialise access.
        conn: Mutex<Connect>,
    }

    // SAFETY: all access to the libvirt connection goes through the Mutex.
    unsafe impl Send for LibvirtHypervisor {}
    unsafe impl Sync for LibvirtHypervisor {}

    impl LibvirtHypervisor {
        pub fn connect(uri: &str) -> BResult<Self> {
            let conn = Connect::open(Some(uri)).map_err(|e| BackendError::Other(e.to_string()))?;
            return Ok(LibvirtHypervisor {
                conn: Mutex::new(conn),
            });
        }

        fn with_dom<T>(
            &self,
            name: &str,
            f: impl FnOnce(&Domain) -> Result<T, virt::error::Error>,
        ) -> BResult<T> {
            let conn = self.conn.lock().unwrap();
            let dom = Domain::lookup_by_name(&conn, name)
                .map_err(|_| BackendError::NotFound(name.into()))?;
            return f(&dom).map_err(|e| BackendError::Other(e.to_string()));
        }
    }

    impl Hypervisor for LibvirtHypervisor {
        fn state(&self, d: &str) -> BResult<VmState> {
            return self.with_dom(d, |dom| {
                let (st, _reason) = dom.get_state()?;
                Ok(match st {
                    sys::VIR_DOMAIN_RUNNING => VmState::Running,
                    sys::VIR_DOMAIN_PAUSED | sys::VIR_DOMAIN_PMSUSPENDED => VmState::Paused,
                    sys::VIR_DOMAIN_SHUTOFF | sys::VIR_DOMAIN_SHUTDOWN => VmState::ShutOff,
                    sys::VIR_DOMAIN_CRASHED => VmState::Crashed,
                    _ => VmState::Unknown,
                })
            });
        }
        fn start(&self, d: &str) -> BResult<()> {
            return self.with_dom(d, |dom| dom.create().map(|_| ()));
        }
        fn shutdown(&self, d: &str) -> BResult<()> {
            return self.with_dom(d, |dom| dom.shutdown().map(|_| ()));
        }
        fn pause(&self, d: &str) -> BResult<()> {
            return self.with_dom(d, |dom| dom.suspend().map(|_| ()));
        }
        fn resume(&self, d: &str) -> BResult<()> {
            return self.with_dom(d, |dom| dom.resume().map(|_| ()));
        }
    }
}
