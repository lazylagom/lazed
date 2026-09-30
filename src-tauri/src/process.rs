//! Bounded process capture. The deadline covers the child and its pipes,
//! including descendants retaining stdout after the shell has exited.
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Output {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

struct Job(Child);
impl Drop for Job {
    fn drop(&mut self) {
        // process_group(0) gave this child a group of its own; never target
        // the app's group or a pre-existing terminal session.
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL); }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn nonblocking(stream: &impl AsRawFd) -> Result<(), String> {
    let fd = stream.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn drain(stream: &mut impl Read, bytes: &mut Vec<u8>, cap: usize) -> Result<bool, String> {
    let mut chunk = [0u8; 8192];
    // Bound each drain too, so an infinite producer cannot starve the
    // deadline or the other pipe.
    for _ in 0..8 {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                if bytes.len().saturating_add(n) > cap { return Err("process output limit exceeded".into()); }
                bytes.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(false)
}

pub fn capture(cmd: &mut Command, timeout: Duration, stdout_cap: usize, stderr_cap: usize) -> Result<Output, String> {
    let deadline = Instant::now() + timeout;
    let child = cmd.process_group(0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("spawn failed: {e}"))?;
    let mut job = Job(child);
    let mut stdout = job.0.stdout.take().ok_or("missing stdout")?;
    let mut stderr = job.0.stderr.take().ok_or("missing stderr")?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_done, mut err_done) = (false, false);
    let mut status = None;
    loop {
        if !out_done { out_done = drain(&mut stdout, &mut out, stdout_cap)?; }
        if !err_done { err_done = drain(&mut stderr, &mut err, stderr_cap)?; }
        if status.is_none() { status = job.0.try_wait().map_err(|e| format!("wait failed: {e}"))?; }
        if let Some(status) = status {
            if out_done && err_done { return Ok(Output { status, stdout: out, stderr: err }); }
        }
        if Instant::now() >= deadline { return Err(format!("timed out after {}s", timeout.as_secs_f64())); }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Stream stdout without accumulating the complete command output. A
/// consumer returning true stops and reaps the job immediately.
pub fn stream_stdout(
    cmd: &mut Command,
    timeout: Duration,
    mut consume: impl FnMut(&[u8]) -> Result<bool, String>,
) -> Result<(Option<std::process::ExitStatus>, Vec<u8>), String> {
    let deadline = Instant::now() + timeout;
    let mut job = Job(cmd.process_group(0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| e.to_string())?);
    let mut stdout = job.0.stdout.take().ok_or("missing stdout")?;
    let mut stderr = job.0.stderr.take().ok_or("missing stderr")?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let mut errors = Vec::new();
    let (mut out_done, mut err_done) = (false, false);
    let mut status = None;
    let mut chunk = [0u8; 8192];
    loop {
        if !out_done {
            for _ in 0..8 {
                match stdout.read(&mut chunk) {
                    Ok(0) => { out_done = true; break; }
                    Ok(n) => if consume(&chunk[..n])? { return Ok((None, errors)); },
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.to_string()),
                }
            }
        }
        if !err_done { err_done = drain(&mut stderr, &mut errors, 1024 * 1024)?; }
        if status.is_none() { status = job.0.try_wait().map_err(|e| e.to_string())?; }
        if let Some(status) = status {
            if out_done && err_done { return Ok((Some(status), errors)); }
        }
        if Instant::now() >= deadline { return Err("search timed out".into()); }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_covers_descendants_and_exited_shells() {
        for script in ["sleep 10 & wait", "sleep 10 &"] {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", script]);
            let start = Instant::now();
            let result = capture(&mut cmd, Duration::from_millis(200), 1024, 1024);
            assert!(result.err().unwrap().contains("timed out"));
            assert!(start.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    fn noisy_output_is_bounded_and_both_pipes_are_drained() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "while :; do printf '1234567890'; printf 'error' >&2; done"]);
        let result = capture(&mut cmd, Duration::from_secs(2), 4096, 4096);
        assert!(result.err().unwrap().contains("output limit"));
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'out'; printf 'err' >&2"]);
        let out = capture(&mut cmd, Duration::from_secs(2), 4096, 4096).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
    }
}
