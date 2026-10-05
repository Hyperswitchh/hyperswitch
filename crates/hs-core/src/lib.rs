//! Hyperswitch core library.
//!
//! The daemon is a thin shell around these modules:
//!
//! - [`config`]: parses `/etc/hyperswitch/hyperswitch.toml`
//! - [`backend`]: the [`backend::Hypervisor`] trait (libvirt in prod, mock in tests)
//! - [`input`]: hotkey detection + keyboard/mouse routing to the active OS
//! - [`hooks`]: display / audio switching via templated host commands
//! - [`switcher`]: the switch engine that orchestrates all of the above
//! - [`ipc`]: JSON-lines protocol spoken over the daemon's Unix socket

pub mod backend;
pub mod config;
pub mod hooks;
pub mod input;
pub mod ipc;
pub mod switcher;

pub use backend::{Hypervisor, VmState};
pub use config::Config;
pub use switcher::{SwitchReport, Switcher};

/// Default location of the daemon's control socket.
pub const SOCKET_PATH: &str = "/run/hyperswitch/hyperswitch.sock";
/// Default location of the config file.
pub const CONFIG_PATH: &str = "/etc/hyperswitch/hyperswitch.toml";
