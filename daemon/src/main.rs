//! lazed — the lazy-workspace daemon.
//!
//!   lazed server [--foreground]       start the daemon
//!   lazed api <method> [params-json]  one-shot API call over the socket
//!   lazed task | inbox                durable tasks, GTD inbox
//!   lazed herdr status|snapshot|call <method> [params-json]
//!                                     herdr adapter — talks to the herdr
//!                                     server directly, no lazed daemon needed
//!   lazed install | uninstall | doctor
//!                                     manage the CLI link on $HOME
//!   lazed status | stop | restart     convenience wrappers
mod herdr;
#[path = "../../shared/repo.rs"]
mod repo;
mod inbox;
mod install;
mod server;
mod session;
mod specs;
mod state;
mod tasks;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("server") => cmd_server(&args[1..]),
        Some("api") => cmd_api(&args[1..]),
        Some("task") => tasks::cli(&args[1..]),
        Some("inbox") => inbox::cli(&args[1..]),
        Some(group @ ("agent" | "pane" | "worktree")) => {
            eprintln!(
                "lazed {group}: moved to herdr — use `herdr {group} …` (see `herdr --skill`)"
            );
            2
        }
        Some("herdr") => cmd_herdr(&args[1..]),
        Some("install") => install::install(&args[1..]),
        Some("uninstall") => install::uninstall(&args[1..]),
        Some("doctor") => install::doctor(&args[1..]),
        Some("status") => cmd_api(&["session.status".into()]),
        Some("stop") => cmd_api(&["server.stop".into()]),
        Some("restart") => cmd_restart(),
        Some("--version") | Some("-V") => {
            println!("lazed {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("--help") | Some("-h") | None => {
            print_help();
            0
        }
        Some(other) => {
            eprintln!("lazed: unknown command '{other}'");
            print_help();
            2
        }
    };
    std::process::exit(code);
}

fn print_help() {
    eprintln!(
        "lazed — lazy-workspace daemon

USAGE:
  lazed server [--foreground]     start the daemon
  lazed api <method> [params]     one-shot API call (params = JSON)
  lazed task <start|status|list|read|tell|resume> [options]
  lazed inbox <add|list|done|reopen|snooze|remove>
  (pane/agent/worktree control: `herdr pane …`, `herdr agent …`, `herdr worktree …`)
  lazed herdr <status|snapshot|call <method> [params]>
                                  herdr execution-layer adapter (PLAN §3b)
  lazed install | uninstall | doctor
  lazed status | stop | restart
  lazed --version"
    );
}

fn cmd_server(args: &[String]) -> i32 {
    // The GUI spawns us with stdio null'd — keep diagnostics by pointing
    // stderr at the state-dir log unless --foreground is requested.
    if !args.iter().any(|a| a == "--foreground") {
        let _ = state::ensure_dir();
        if let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state::log_path())
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::dup2(f.as_raw_fd(), 2);
            }
        }
    }
    match server::run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lazed server: {e}");
            1
        }
    }
}

/// Stop the running daemon (if any), wait for the socket to go quiet, then
/// spawn a fresh detached `lazed server` from this binary. Panes are
/// respawned from the persisted session; processes inside them are lost.
fn cmd_restart() -> i32 {
    let was_running = api_call("server.stop", json!({})).is_ok();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while was_running && std::time::Instant::now() < deadline {
        if api_call("session.status", json!({})).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if was_running && api_call("session.status", json!({})).is_ok() {
        eprintln!("lazed restart: old server did not exit");
        return 1;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lazed restart: cannot resolve own binary: {e}");
            return 1;
        }
    };
    if let Err(e) = std::process::Command::new(exe)
        .arg("server")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        eprintln!("lazed restart: failed to spawn server: {e}");
        return 1;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Ok(v) = api_call("session.status", json!({})) {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    eprintln!("lazed restart: new server did not become ready within 10s");
    1
}

/// Open a socket, send one request, print the first response line.
fn api_call(method: &str, params: Value) -> Result<Value, String> {
    let path = state::sock_path();
    let mut s = UnixStream::connect(&path)
        .map_err(|e| format!("cannot connect {}: {e}", path.display()))?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(660))).map_err(|e| e.to_string())?;
    let req = json!({"id": 1, "method": method, "params": params});
    writeln!(s, "{}", serde_json::to_string(&req).unwrap()).map_err(|e| e.to_string())?;
    s.flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s.try_clone().map_err(|e| e.to_string())?)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
    if let Some(e) = v.get("error") {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

fn cmd_api(args: &[String]) -> i32 {
    let Some(method) = args.first() else {
        eprintln!("usage: lazed api <method> [params-json]");
        return 2;
    };
    let params: Value = match args.get(1) {
        Some(raw) => match serde_json::from_str(raw) {
            Ok(value) => value,
            Err(e) => { eprintln!("invalid params JSON: {e}"); return 2; }
        },
        None => json!({}),
    };
    match api_call(method, params) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            0
        }
        Err(e) => {
            eprintln!("lazed api: {e}");
            1
        }
    }
}


/// `lazed herdr …` — exercise the herdr adapter without the lazed daemon
/// (status/snapshot/call go straight to the herdr socket). Used by the
/// app's health check and by hand when debugging the Path C seam.
fn cmd_herdr(args: &[String]) -> i32 {
    let out = match args.first().map(String::as_str) {
        Some("status") => Ok(herdr::status_json()),
        Some("snapshot") => herdr::snapshot(),
        Some("call") => match args.get(1) {
            None => Err("usage: lazed herdr call <method> [params-json]".to_string()),
            Some(method) => {
                let params = match args.get(2) {
                    None => json!({}),
                    Some(raw) => match serde_json::from_str::<Value>(raw) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!("lazed herdr: params is not JSON: {e}");
                            return 2;
                        }
                    },
                };
                herdr::call(method, params)
            }
        },
        _ => Err("usage: lazed herdr <status|snapshot|call <method> [params]>".to_string()),
    };
    match out {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("lazed herdr: {e}");
            1
        }
    }
}
