//! hyperswitchd: runs as root on the Hyperswitch host, started by systemd.
//!
//! Threads/tasks:
//!   - input threads (one per physical device)  → HotkeyAction over std mpsc
//!   - hotkey bridge task                       → Switcher::handle
//!   - socket server task                       → Switcher (CLI / overlay UI)

use anyhow::{Context, Result};
use clap::Parser;
use hs_core::{
    backend::{Hypervisor, MockHypervisor},
    hooks::{Hooks, NoHooks, ShellHooks},
    input::{HotkeyAction, InputRouter, NullRouter},
    ipc::{Payload, Request, Response},
    Config, Switcher, CONFIG_PATH, SOCKET_PATH,
};
use std::{path::PathBuf, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

#[derive(Parser)]
#[command(name = "hyperswitchd", version, about = "Hyperswitch host daemon")]
struct Args {
    #[arg(short, long, default_value = CONFIG_PATH)]
    config: PathBuf,
    #[arg(short, long, default_value = SOCKET_PATH)]
    socket: PathBuf,
    /// Use the in-memory hypervisor and no input grabbing (dev / CI).
    #[arg(long)]
    mock: bool,
    #[arg(long, default_value = "qemu:///system")]
    libvirt_uri: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let cfg = Config::load(&args.config)?;
    tracing::info!(oses = cfg.oses.len(), "config loaded");

    let (tx, rx) = std::sync::mpsc::channel::<HotkeyAction>();
    let hv = hypervisor(&args, &cfg)?;
    let input = router(&args, &cfg, tx)?;
    let hooks: Arc<dyn Hooks> = if args.mock {
        Arc::new(NoHooks)
    } else {
        Arc::new(ShellHooks {
            on_focus: cfg.switch.on_focus.clone(),
            on_blur: cfg.switch.on_blur.clone(),
        })
    };

    let switcher = Arc::new(Switcher::new(cfg, hv, input, hooks));
    {
        let s = switcher.clone();
        tokio::task::spawn_blocking(move || s.boot_all()).await??;
    }

    // Hotkeys → switch engine. Blocking recv lives on its own thread.
    {
        let s = switcher.clone();
        std::thread::Builder::new().name("hs-hotkeys".into()).spawn(move || {
            while let Ok(action) = rx.recv() {
                match s.handle(action) {
                    Ok(r) => tracing::info!(from = %r.from, to = %r.to, us = r.elapsed_us, "switched"),
                    Err(e) => tracing::error!(%e, "switch failed"),
                }
            }
        })?;
    }

    return serve(&args.socket, switcher).await;
}

fn hypervisor(args: &Args, cfg: &Config) -> Result<Arc<dyn Hypervisor>> {
    if args.mock {
        return Ok(Arc::new(MockHypervisor::with_domains(
            cfg.oses.iter().map(|o| o.domain.as_str()),
        )));
    }
    #[cfg(feature = "libvirt")]
    {
        let hv = hs_core::backend::libvirt::LibvirtHypervisor::connect(&args.libvirt_uri)?;
        return Ok(Arc::new(hv));
    }
    #[allow(unreachable_code)]
    {
        let _ = &args.libvirt_uri;
        anyhow::bail!(
            "built without `libvirt` feature; run with --mock or rebuild with --features host"
        )
    }
}

fn router(
    args: &Args,
    cfg: &Config,
    tx: std::sync::mpsc::Sender<HotkeyAction>,
) -> Result<Arc<dyn InputRouter>> {
    if args.mock || cfg.switch.input_devices.is_empty() {
        drop(tx);
        return Ok(Arc::new(NullRouter::default()));
    }
    #[cfg(feature = "evdev")]
    {
        use hs_core::input::{evdev_router::EvdevRouter, HotkeyMatcher};
        let h = &cfg.hotkeys;
        let matcher = HotkeyMatcher::new(&h.next, &h.prev, &h.direct_modifier, cfg.oses.len());
        let ids: Vec<String> = cfg.oses.iter().map(|o| o.id.clone()).collect();
        return Ok(EvdevRouter::spawn(
            &cfg.switch.input_devices,
            &ids,
            matcher,
            tx,
        )?);
    }
    #[allow(unreachable_code)]
    {
        drop(tx);
        tracing::warn!("built without `evdev`; hotkeys disabled, use `hyperswitch switch <id>`");
        return Ok(Arc::new(NullRouter::default()));
    }
}

async fn serve(path: &PathBuf, switcher: Arc<Switcher>) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // sockaddr_un.sun_path is 108 bytes including the trailing NUL.
    if path.as_os_str().len() > 107 {
        anyhow::bail!(
            "socket path is {} bytes, the Unix limit is 107: {}",
            path.as_os_str().len(),
            path.display()
        );
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).with_context(|| format!("bind {}", path.display()))?;
    tracing::info!(socket = %path.display(), "listening");

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let s = switcher.clone();
                tokio::spawn(async move {
                    if let Err(e) = client(stream, s).await {
                        tracing::debug!(%e, "client closed");
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down (guests keep running)");
                let _ = std::fs::remove_file(path);
                return Ok(());
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM, shutting down (guests keep running)");
                let _ = std::fs::remove_file(path);
                return Ok(());
            }
        }
    }
}

async fn client(stream: UnixStream, s: Arc<Switcher>) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    while let Some(line) = lines.next_line().await? {
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let s = s.clone();
                tokio::task::spawn_blocking(move || dispatch(&s, req)).await?
            }
            Err(e) => Response::err(format!("bad request: {e}")),
        };
        let mut out = serde_json::to_vec(&resp)?;
        out.push(b'\n');
        w.write_all(&out).await?;
    }
    return Ok(());
}

fn dispatch(s: &Switcher, req: Request) -> Response {
    let r = match req {
        Request::Status => return Response::ok(Payload::Status(s.status())),
        Request::Switch { id } => s.switch_to(&id).map(Payload::Switched),
        Request::Next => s.handle(HotkeyAction::Next).map(Payload::Switched),
        Request::Prev => s.handle(HotkeyAction::Prev).map(Payload::Switched),
        Request::Start { id } => s.start(&id).map(|_| Payload::Done),
        Request::Stop { id } => s.stop(&id).map(|_| Payload::Done),
    };
    return r.map(Response::ok).unwrap_or_else(Response::err);
}
