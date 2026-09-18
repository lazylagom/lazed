//! lazed — the lazy-workspace daemon.
//!
//!   lazed server [--foreground]       start the daemon
//!   lazed api <method> [params-json]  one-shot API call over the socket
//!   lazed term attach <id> [--cols N --rows N]
//!                                     attach a control stream: JSON cmds
//!                                     on stdin, frames/events on stdout
//!   lazed install | uninstall | doctor
//!                                     manage the CLI/skill links on $HOME
//!   lazed status | stop | restart     convenience wrappers
mod agent;
mod control;
mod history;
mod inbox;
mod install;
mod named;
mod cli;
mod tasks;
mod render;
mod server;
mod session;
mod state;
mod term;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("server") => cmd_server(&args[1..]),
        Some("api") => cmd_api(&args[1..]),
        Some("task") => tasks::cli(&args[1..]),
        Some("inbox") => inbox::cli(&args[1..]),
        Some(group @ ("agent" | "pane" | "worktree")) => cli::run(group, &args[1..]),
        Some("term") => cmd_term(&args[1..]),
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
  lazed agent | pane | worktree   discover agent and layout commands
  lazed term attach <term_id>     control stream (JSON in, frames out)
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

fn cmd_term(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("attach") => term_attach(&args[1..]),
        _ => {
            eprintln!("usage: lazed term attach <term_id> [--cols N] [--rows N]");
            2
        }
    }
}

fn term_attach(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("usage: lazed term attach <term_id> [--cols N] [--rows N]");
        return 2;
    };
    let mut cols = 80u64;
    let mut rows = 24u64;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--cols" => {
                cols = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(80);
                i += 2;
            }
            "--rows" => {
                rows = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(24);
                i += 2;
            }
            _ => i += 1,
        }
    }
    let path = state::sock_path();
    let sock = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("lazed: cannot connect {}: {e}", path.display());
            return 1;
        }
    };
    let mut w = match sock.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("lazed: {e}");
            return 1;
        }
    };
    // attach request — then this channel carries term.* events + commands
    let req = json!({
        "method": "terminal.attach",
        "params": {"term_id": id, "cols": cols, "rows": rows},
    });
    if writeln!(w, "{}", serde_json::to_string(&req).unwrap()).is_err() {
        return 1;
    }
    // stdin → socket: raw {"type": ...} commands
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        let mut buf = [0u8; 65536];
        loop {
            match lock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if w.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = w.flush();
                }
                Err(_) => break,
            }
        }
        let _ = w.shutdown(std::net::Shutdown::Write);
    });
    // socket → stdout: every pushed line verbatim
    let mut r = BufReader::new(sock);
    let mut out = std::io::stdout().lock();
    let mut line = String::new();
    loop {
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if out.write_all(line.as_bytes()).is_err() {
                    break;
                }
                let _ = out.flush();
            }
            Err(_) => break,
        }
    }
    0
}
