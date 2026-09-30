use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde_json::{json, Value};

/// Cap on emitted entries — a pathological checkout (e.g. an untracked
/// vendor dir git doesn't ignore) must not flood the IPC payload.
const MAX_ENTRIES: usize = 50_000;
/// `fs_read` reads at most this much of a file for the peek overlay.
const READ_CAP: u64 = 512 * 1024;
/// `fs_search` returns at most this many hits.
const MAX_HITS: usize = 300;

fn cmd(dir: &str, prog: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new(prog);
    command.env("GIT_OPTIONAL_LOCKS", "0").arg("-C").arg(dir).args(args);
    let out = crate::process::capture(&mut command, std::time::Duration::from_secs(30), 8 * 1024 * 1024, 1024 * 1024)?;
    if !out.status.success() {
        return Err(format!(
            "{prog} {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn split_z(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Parse `git status --porcelain=v1 -z -uall --ignored=matching` output.
/// Returns (path → badge, dir paths). Badges: "U" untracked, "!" ignored,
/// "D" deleted, "M" everything else changed. A trailing slash marks a dir
/// git won't descend into (ignored dir or embedded repo) — it's listed
/// collapsed. With -z a rename/copy record is `XY to\0from\0` — `to` gets
/// the badge.
pub(crate) fn parse_porcelain_z(
    bytes: &[u8],
) -> (BTreeMap<String, String>, BTreeSet<String>) {
    let fields = split_z(bytes);
    let mut badges = BTreeMap::new();
    let mut dirs = BTreeSet::new();
    let mut i = 0;
    while i < fields.len() {
        let rec = &fields[i];
        i += 1;
        if rec.len() < 4 {
            continue;
        }
        let x = rec.as_bytes()[0] as char;
        let y = rec.as_bytes()[1] as char;
        let mut path = rec[3..].to_string();
        if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
            // the record's second NUL field is the source path — consume it
            i += 1;
        }
        let badge = match (x, y) {
            ('?', '?') => "U",
            ('!', '!') => "!",
            _ if x == 'D' || y == 'D' => "D",
            _ => "M",
        };
        if path.ends_with('/') {
            path.pop();
            dirs.insert(path.clone());
        }
        badges.insert(path, badge.to_string());
    }
    (badges, dirs)
}

/// Files + dirs of a checkout, gitignore-aware. Git mode lists tracked +
/// untracked (ignored dirs appear collapsed, flagged "!"); a non-repo root
/// falls back to a bounded plain walk. Dirs are emitted explicitly so the
/// client never has to guess at empty/ignored containers.
pub fn tree(root: &str) -> Result<Value, String> {
    let root_path = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    let root = root_path.to_string_lossy().to_string();

    let in_repo = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false);

    let mut files: BTreeSet<String> = BTreeSet::new();
    // dirs the backend lists without contents: ignored dirs, embedded
    // repos (git won't cross the boundary) — and every dir in the walk
    // fallback, which does enumerate
    let mut collapsed: BTreeSet<String> = BTreeSet::new();
    let mut extra_dirs: BTreeSet<String> = BTreeSet::new();
    let mut badges: BTreeMap<String, String> = BTreeMap::new();
    let mut git_root = root.clone();

    if in_repo {
        // status paths are toplevel-relative — resolve the real root first
        if let Ok(out) = cmd(&root, "git", &["rev-parse", "--show-toplevel"]) {
            git_root = String::from_utf8_lossy(&out).trim().to_string();
        }
        if let Ok(out) = cmd(
            &git_root,
            "git",
            &["ls-files", "-z", "-c", "-o", "--exclude-standard"],
        ) {
            for p in split_z(&out) {
                if files.len() >= MAX_ENTRIES {
                    break;
                }
                // a trailing slash = a dir git refuses to descend into
                if let Some(dir) = p.strip_suffix('/') {
                    extra_dirs.insert(dir.to_string());
                    collapsed.insert(dir.to_string());
                } else {
                    files.insert(p);
                }
            }
        }
        if let Ok(out) = cmd(
            &git_root,
            "git",
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "-uall",
                "--ignored=matching",
            ],
        ) {
            let (b, slash_dirs) = parse_porcelain_z(&out);
            for d in &slash_dirs {
                extra_dirs.insert(d.clone());
                collapsed.insert(d.clone());
            }
            for (p, _) in &b {
                // status also surfaces deleted tracked files; the union with
                // ls-files keeps the row so its D badge has somewhere to sit
                if files.len() >= MAX_ENTRIES {
                    break;
                }
                if !slash_dirs.contains(p) {
                    files.insert(p.clone());
                }
            }
            badges = b;
        }
    } else {
        // non-repo fallback: bounded walk, .git excluded
        let mut stack = vec![root_path.clone()];
        while let Some(dir) = stack.pop() {
            if files.len() + extra_dirs.len() >= MAX_ENTRIES {
                break;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name == ".git" {
                    continue;
                }
                let rel = match p.strip_prefix(&root_path) {
                    Ok(r) => r.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };
                if p.is_dir() {
                    extra_dirs.insert(rel);
                    stack.push(p);
                } else {
                    files.insert(rel);
                }
            }
        }
    }

    // roll each path's badge up to its ancestor dirs — a dir shows the
    // worst descendant state (M/D > U > !)
    let rank = |b: &str| match b {
        "M" | "D" => 3,
        "U" => 2,
        _ => 1,
    };
    for (p, b) in badges.clone() {
        let mut anc = p.as_str();
        while let Some((parent, _)) = anc.rsplit_once('/') {
            let cur = badges.get(parent).map(String::as_str).unwrap_or("");
            if rank(&b) > rank(cur) {
                badges.insert(parent.to_string(), b.clone());
            }
            anc = parent;
        }
    }

    // derive every ancestor dir of every listed file
    let mut dirs: BTreeSet<String> = extra_dirs;
    for f in &files {
        let mut anc = f.as_str();
        while let Some((parent, _)) = anc.rsplit_once('/') {
            dirs.insert(parent.to_string());
            anc = parent;
        }
    }

    let mut entries: Vec<Value> = Vec::with_capacity(dirs.len() + files.len());
    for d in &dirs {
        entries.push(json!({
            "path": d,
            "kind": "dir",
            "git": badges.get(d),
            "collapsed": collapsed.contains(d),
        }));
    }
    for f in &files {
        entries.push(json!({
            "path": f,
            "kind": "file",
            "git": badges.get(f),
        }));
    }
    let truncated = files.len() + dirs.len() >= MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    Ok(json!({
        "root": if in_repo { &git_root } else { &root },
        "git": in_repo,
        "truncated": truncated,
        "entries": entries,
    }))
}

/// Peek at a file: UTF-8 text up to READ_CAP, or `{binary}` for anything
/// with a NUL in the first 8KB.
pub fn read_file(path: &str) -> Result<Value, String> {
    let p = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !p.is_file() {
        return Err(format!("not a file: {path}"));
    }
    use std::io::Read;
    let mut f = std::fs::File::open(&p).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; (READ_CAP + 1) as usize];
    let n = f.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(n);
    let truncated = n as u64 > READ_CAP;
    if truncated {
        buf.truncate(READ_CAP as usize);
    }
    if buf[..buf.len().min(8192)].contains(&0) {
        return Ok(json!({"binary": true}));
    }
    Ok(json!({
        "content": String::from_utf8_lossy(&buf),
        "truncated": truncated,
    }))
}

/// `git diff HEAD -- <path>` — staged + unstaged vs HEAD. Unborn HEAD
/// (no commits yet) falls back to `diff` + `diff --cached`.
pub fn file_diff(root: &str, path: &str) -> Result<Value, String> {
    let diff = cmd(root, "git", &["diff", "HEAD", "--", path]).or_else(|_| {
        let mut a = cmd(root, "git", &["diff", "--", path])?;
        a.extend_from_slice(&cmd(root, "git", &["diff", "--cached", "--", path])?);
        Ok::<Vec<u8>, String>(a)
    })?;
    Ok(json!({"diff": String::from_utf8_lossy(&diff)}))
}

/// Content search — rg when present (gitignore-aware, hidden included,
/// `.git` still excluded), else `git grep --untracked` which covers
/// tracked + untracked-nonignored files.
pub fn search(root: &str, query: &str) -> Result<Value, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(json!({"results": [], "truncated": false}));
    }
    let checkout = crate::repo::resolve(root).map(|r| r.checkout).unwrap_or_else(|| root.to_string());
    let mut rg = Command::new("rg");
    rg.args(["--json", "--no-messages", "--hidden", "-i", "-m", "5", "--max-columns", "300", "-g", "!.git", "-e", q]).current_dir(&checkout);
    // Only a missing rg triggers fallback; invalid expressions and IO
    // errors must not silently become a different search.
    match search_command(&mut rg, true) {
        Err(e) if e.contains("No such file or directory") => {
            let mut git = Command::new("git");
            git.arg("-C").arg(&checkout).args(["grep", "--full-name", "-nIiz", "--untracked", "-e", q]);
            search_command(&mut git, false)
        }
        result => result,
    }
}

fn search_command(cmd: &mut Command, json_lines: bool) -> Result<Value, String> {
    let mut pending = Vec::new();
    let mut results = Vec::new();
    let mut truncated = false;
    let (status, errors) = crate::process::stream_stdout(cmd, std::time::Duration::from_secs(30), |chunk| {
        pending.extend_from_slice(chunk);
        if pending.len() > 1024 * 1024 { return Err("search record exceeds 1MB".into()); }
        loop {
            let end = if json_lines {
                pending.iter().position(|b| *b == b'\n')
            } else {
                // Newlines in the file name are valid. Find the content
                // terminator only after the two NUL-delimited fields.
                let fields: Vec<_> = pending.iter().enumerate().filter(|(_, b)| **b == 0).take(2).map(|(i, _)| i).collect();
                fields.get(1).and_then(|start| pending[start + 1..].iter().position(|b| *b == b'\n').map(|i| start + 1 + i))
            };
            let Some(end) = end else { break };
            let hit = parse_search_record(&pending[..end], json_lines)?;
            pending.drain(..=end);
            if let Some(hit) = hit {
                if results.len() == MAX_HITS { truncated = true; return Ok(true); }
                results.push(hit);
            }
        }
        Ok(false)
    })?;
    if !truncated && !pending.is_empty() {
        if let Some(hit) = parse_search_record(&pending, json_lines)? {
            if results.len() == MAX_HITS { truncated = true; } else { results.push(hit); }
        }
    }
    if status.is_some_and(|s| !s.success() && s.code() != Some(1)) {
        return Err(format!("search failed: {}", String::from_utf8_lossy(&errors).chars().take(300).collect::<String>()));
    }
    Ok(json!({"results": results, "truncated": truncated}))
}

fn parse_search_record(bytes: &[u8], json_lines: bool) -> Result<Option<Value>, String> {
    if json_lines {
        let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if value["type"] != "match" { return Ok(None); }
        let Some(path) = value.pointer("/data/path/text").and_then(Value::as_str) else { return Ok(None) };
        let text = value.pointer("/data/lines/text").and_then(Value::as_str).unwrap_or("");
        return Ok(Some(json!({"path": path.strip_prefix("./").unwrap_or(path), "line": value["data"]["line_number"], "text": text.trim()})));
    }
    let mut fields = bytes.splitn(3, |b| *b == 0);
    let path = fields.next().unwrap_or_default();
    let Some(number) = fields.next() else { return Ok(None) };
    let Some(text) = fields.next() else { return Ok(None) };
    let line: u32 = String::from_utf8_lossy(number).parse().map_err(|e| format!("invalid search line: {e}"))?;
    Ok(Some(json!({"path": String::from_utf8_lossy(path), "line": line, "text": String::from_utf8_lossy(text).trim()})))
}

/// Open a file in the desktop's default app (preview overlay's "open" button).
pub fn open_path(path: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    Command::new(opener)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_porcelain_z;

    #[test]
    fn porcelain_parses_statuses() {
        let out = b" M src/App.tsx\0?? new.ts\0!! dist/\0R  dst.ts\0src.ts\0 D gone.ts\0?? .claude/wt/\0";
        let (badges, dirs) = parse_porcelain_z(out);
        assert_eq!(badges.get("src/App.tsx").map(String::as_str), Some("M"));
        assert_eq!(badges.get("new.ts").map(String::as_str), Some("U"));
        assert_eq!(badges.get("dist").map(String::as_str), Some("!"));
        assert_eq!(badges.get("dst.ts").map(String::as_str), Some("M"));
        assert_eq!(badges.get("gone.ts").map(String::as_str), Some("D"));
        // the rename source field was consumed, not reported as a path
        assert!(!badges.contains_key("src.ts"));
        // trailing-slash dirs collected — ignored dir + embedded repo alike
        assert!(dirs.contains("dist"));
        assert!(dirs.contains(".claude/wt"));
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn porcelain_handles_empty_and_short() {
        let (b, d) = parse_porcelain_z(b"");
        assert!(b.is_empty() && d.is_empty());
        let (b, d) = parse_porcelain_z(b" M \0 x\0");
        assert!(b.is_empty() && d.is_empty());
    }

    /// tree() over a real temp repo — exercises git ls-files + porcelain
    /// end to end (tracked/clean, modified, untracked, ignored dir).
    #[test]
    fn tree_lists_real_repo() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("lazed-fstree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn a() {}").unwrap();
        std::fs::write(dir.join("b.md"), "hi").unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(dir.join("ignored/x"), "x").unwrap();
        let g = |args: &[&str]| {
            let o = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap();
            assert!(o.status.success(), "git {args:?}: {:?}", o.stderr);
        };
        g(&["init", "-q"]);
        g(&["add", "src/a.rs", "b.md"]);
        g(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ]);
        std::fs::write(dir.join("src/a.rs"), "changed").unwrap();
        std::fs::write(dir.join("new.txt"), "n").unwrap();

        let v = super::tree(dir.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(v["git"], true);
        let entries = v["entries"].as_array().unwrap();
        let get = |p: &str| {
            entries
                .iter()
                .find(|e| e["path"].as_str() == Some(p))
                .unwrap_or_else(|| panic!("missing entry {p}"))
                .clone()
        };
        assert_eq!(get("src")["kind"].as_str(), Some("dir"));
        assert_eq!(get("src")["git"].as_str(), Some("M"));
        assert_eq!(get("src/a.rs")["git"].as_str(), Some("M"));
        assert_eq!(get("new.txt")["git"].as_str(), Some("U"));
        assert_eq!(get(".gitignore")["git"].as_str(), Some("U"));
        assert_eq!(get("b.md")["git"], serde_json::Value::Null); // clean
        let ig = get("ignored");
        assert_eq!(ig["git"].as_str(), Some("!"));
        assert_eq!(ig["collapsed"], true);
        assert!(entries.iter().all(|e| e["path"].as_str() != Some("ignored/x")));
    }
}

#[cfg(test)]
mod search_regressions {
    use super::*;
    #[test]
    fn structured_search_preserves_colons_and_newlines_in_paths_and_subdirectory_roots() {
        let fixture = crate::repo::tests::fixture();
        std::fs::create_dir_all(fixture.linked.join("src")).unwrap();
        for name in ["src/plain.txt", "src/with:colon.txt", "src/with\nnewline.txt"] {
            std::fs::write(fixture.linked.join(name), "needle\n").unwrap();
        }
        let root = fixture.linked.join("src");
        let tree = tree(root.to_str().unwrap()).unwrap();
        assert_eq!(tree["root"], fixture.linked.to_str().unwrap());
        let results = search(root.to_str().unwrap(), "needle").unwrap();
        assert_eq!(results["results"].as_array().unwrap().len(), 3);
        let mut git = Command::new("git");
        git.arg("-C").arg(&fixture.linked).args(["grep", "--full-name", "-nIiz", "--untracked", "-e", "needle"]);
        let fallback = search_command(&mut git, false).unwrap();
        for name in ["src/plain.txt", "src/with:colon.txt", "src/with\nnewline.txt"] {
            assert!(results["results"].as_array().unwrap().iter().any(|hit| hit["path"] == name));
            assert!(fallback["results"].as_array().unwrap().iter().any(|hit| hit["path"] == name));
        }
    }
    #[test]
    fn search_stops_after_the_first_hit_beyond_the_limit() {
        let fixture = crate::repo::tests::fixture();
        for i in 0..70 { std::fs::write(fixture.main.join(format!("{i}.txt")), "needle\n".repeat(20)).unwrap(); }
        let value = search(fixture.main.to_str().unwrap(), "needle").unwrap();
        assert_eq!(value["results"].as_array().unwrap().len(), MAX_HITS);
        assert_eq!(value["truncated"], true);
        assert_eq!(search(fixture.main.to_str().unwrap(), "nothingmatches").unwrap()["results"], json!([]));
    }
}
