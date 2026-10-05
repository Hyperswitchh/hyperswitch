//! `hyperswitch status | switch <id> | next | prev | start <id> | stop <id>`

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use hs_core::{
    ipc::{Payload, Request, Response},
    SOCKET_PATH,
};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
};

#[derive(Parser)]
#[command(
    name = "hyperswitch",
    version,
    about = "Control the Hyperswitch daemon"
)]
struct Cli {
    #[arg(short, long, default_value = SOCKET_PATH, global = true)]
    socket: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List OSes, their state, and which one has focus
    Status,
    /// Switch focus to an OS by id
    Switch {
        id: String,
    },
    Next,
    Prev,
    /// Boot an OS in the background
    Start {
        id: String,
    },
    /// Gracefully shut an OS down
    Stop {
        id: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let req = match cli.cmd {
        Cmd::Status => Request::Status,
        Cmd::Switch { id } => Request::Switch { id },
        Cmd::Next => Request::Next,
        Cmd::Prev => Request::Prev,
        Cmd::Start { id } => Request::Start { id },
        Cmd::Stop { id } => Request::Stop { id },
    };

    let mut stream = UnixStream::connect(&cli.socket).with_context(|| {
        format!(
            "cannot reach hyperswitchd at {} (is it running?)",
            cli.socket
        )
    })?;
    let mut line = serde_json::to_string(&req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;

    let mut buf = String::new();
    BufReader::new(stream).read_line(&mut buf)?;
    let resp: Response = serde_json::from_str(&buf)?;
    if !resp.ok {
        bail!(resp.error.unwrap_or_default());
    }
    match resp.data {
        Some(Payload::Status(list)) => {
            println!("{:<3}{:<10}{:<20}STATE", "", "ID", "NAME");
            for os in list {
                let mark = if os.active { "▶" } else { "" };
                println!("{:<3}{:<10}{:<20}{:?}", mark, os.id, os.name, os.state);
            }
        }
        Some(Payload::Switched(r)) => {
            let ms = r.elapsed_us as f64 / 1000.0;
            let note = if r.cold_start { " (cold boot)" } else { "" };
            println!("{} → {} in {ms:.2} ms{note}", r.from, r.to);
        }
        _ => println!("ok"),
    }
    return Ok(());
}
