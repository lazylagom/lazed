//! Domain model: session > group? > project > terminal.
//!
//! - session: the running daemon's namespace (persisted to session.json)
//! - group: an optional named collection of projects (sidebar sections)
//! - project: a repo — owns terminals, knows repo_root/repo_key
//! - terminal: one PTY (worktree or plain)
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::state;
use crate::term::{lock, PtyTerm};

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub label: Option<String>,
    pub repo_root: String,
    pub repo_key: String,
    /// ordered terminal ids — index 0 is the first terminal (root checkout)
    pub terminals: Vec<String>,
}

/// A named collection of projects. A project sits in at most one group —
/// membership lives here (ordered), not on the project.
#[derive(Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub label: Option<String>,
    /// ordered project ids in this group
    pub projects: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct PersistedTerm {
    id: String,
    project_id: String,
    cwd: String,
    command: String,
    label: Option<String>,
    kind: String,
    cols: usize,
    rows: usize,
    /// raw output tail — replayed into the screen model on restore
    tail_b64: String,
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    version: u32,
    next_term: u64,
    next_project: u64,
    #[serde(default)]
    next_group: u64,
    focused_project_id: Option<String>,
    projects: Vec<Project>,
    /// display order — absent in session files written before groups
    #[serde(default)]
    groups: Vec<Group>,
    terminals: Vec<PersistedTerm>,
}

pub struct Session {
    pub projects: HashMap<String, Project>,
    pub terminals: HashMap<String, Arc<Mutex<PtyTerm>>>,
    /// ordered groups — display order in the sidebar
    pub groups: Vec<Group>,
    next_term: u64,
    next_project: u64,
    next_group: u64,
    pub focused_project_id: Option<String>,
    /// global event subscribers (events.subscribe)
    pub event_subs: Vec<Sender<Value>>,
}

impl Session {
    pub fn new() -> Self {
        Session {
            projects: HashMap::new(),
            terminals: HashMap::new(),
            groups: Vec::new(),
            next_term: 1,
            next_project: 1,
            next_group: 1,
            focused_project_id: None,
            event_subs: Vec::new(),
        }
    }

    fn alloc_term(&mut self) -> String {
        let id = format!("t{}", self.next_term);
        self.next_term += 1;
        id
    }

    fn alloc_project(&mut self) -> String {
        let id = format!("p{}", self.next_project);
        self.next_project += 1;
        id
    }

    fn alloc_group(&mut self) -> String {
        let id = format!("g{}", self.next_group);
        self.next_group += 1;
        id
    }

    /// Resolve the repo containing `cwd` — returns (repo_root, repo_key).
    /// repo_key = the shared git-common-dir, so a linked worktree path maps
    /// to the same project as its main checkout. Falls back to cwd itself
    /// for non-repo dirs.
    pub fn resolve_repo(cwd: &str) -> (String, String) {
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let root = git(&["rev-parse", "--show-toplevel"]).unwrap_or_else(|| cwd.to_string());
        let key = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .unwrap_or_else(|| root.clone());
        (root, key)
    }

    pub fn create_terminal(
        &mut self,
        project_id: Option<&str>,
        cwd: &str,
        command: &str,
        label: Option<String>,
        kind: &str,
        cols: usize,
        rows: usize,
    ) -> Result<Arc<Mutex<PtyTerm>>, String> {
        let id = self.alloc_term();
        let (term, mut reader) = PtyTerm::spawn(&id, cwd, command, label, kind, cols, rows)
            .map_err(|e| format!("pty spawn failed: {e}"))?;
        let term = Arc::new(Mutex::new(term));
        // reader thread
        {
            let t = term.clone();
            std::thread::spawn(move || PtyTerm::pump(&t, reader.as_mut()));
        }
        // render thread
        {
            let t = term.clone();
            std::thread::spawn(move || PtyTerm::render_loop(&t));
        }
        self.terminals.insert(id.clone(), term.clone());
        if let Some(pid) = project_id {
            if let Some(p) = self.projects.get_mut(pid) {
                p.terminals.push(id.clone());
            }
        }
        self.broadcast_event(
            "terminal.created",
            json!({"term_id": id, "project_id": project_id, "cwd": cwd}),
        );
        Ok(term)
    }

    pub fn create_project(
        &mut self,
        cwd: &str,
        label: Option<String>,
        group_id: Option<&str>,
    ) -> Result<Value, String> {
        let (repo_root, repo_key) = Self::resolve_repo(cwd);
        let id = self.alloc_project();
        self.projects.insert(
            id.clone(),
            Project {
                id: id.clone(),
                label,
                repo_root,
                repo_key,
                terminals: Vec::new(),
            },
        );
        if let Some(gid) = group_id {
            if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) {
                g.projects.push(id.clone());
            }
        }
        // the project's first terminal opens in the root checkout
        let term = self.create_terminal(
            Some(&id),
            &cwd.to_string(),
            "",
            None,
            "plain",
            80,
            24,
        )?;
        self.broadcast_event("project.created", json!({"project_id": id}));
        let tid = lock(&term).id.clone();
        Ok(json!({
            "project": self.project_json(&id),
            "terminal": {"term_id": tid},
        }))
    }

    /// The group a project belongs to, if any.
    pub fn project_group(&self, project_id: &str) -> Option<&Group> {
        self.groups
            .iter()
            .find(|g| g.projects.iter().any(|p| p == project_id))
    }

    pub fn project_json(&self, id: &str) -> Value {
        match self.projects.get(id) {
            Some(p) => json!({
                "project_id": p.id,
                "label": p.label,
                "repo_root": p.repo_root,
                "repo_key": p.repo_key,
                "group_id": self.project_group(id).map(|g| g.id.clone()),
                "terminals": p.terminals,
                "focused": self.focused_project_id.as_deref() == Some(id),
            }),
            None => Value::Null,
        }
    }

    pub fn group_json(&self, g: &Group) -> Value {
        json!({
            "group_id": g.id,
            "label": g.label,
            "projects": g.projects,
        })
    }

    pub fn create_group(&mut self, label: Option<String>) -> Value {
        let id = self.alloc_group();
        self.groups.push(Group {
            id: id.clone(),
            label,
            projects: Vec::new(),
        });
        self.broadcast_event("group.created", json!({"group_id": id}));
        let g = self.groups.iter().find(|g| g.id == id).unwrap();
        self.group_json(g)
    }

    pub fn rename_group(&mut self, id: &str, label: Option<String>) -> Result<(), String> {
        let g = self
            .groups
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or_else(|| format!("no group {id}"))?;
        g.label = label;
        self.broadcast_event("group.updated", json!({"group_id": id}));
        Ok(())
    }

    /// Removing a group never closes its projects — they go back to
    /// ungrouped.
    pub fn remove_group(&mut self, id: &str) -> Result<(), String> {
        let before = self.groups.len();
        self.groups.retain(|g| g.id != id);
        if self.groups.len() == before {
            return Err(format!("no group {id}"));
        }
        self.broadcast_event("group.removed", json!({"group_id": id}));
        Ok(())
    }

    /// Move a project into a group (None = ungroup). A project lives in at
    /// most one group — it's pulled out of wherever it was.
    pub fn assign_project(&mut self, project_id: &str, group_id: Option<&str>) -> Result<(), String> {
        if !self.projects.contains_key(project_id) {
            return Err(format!("no project {project_id}"));
        }
        if let Some(gid) = group_id {
            if !self.groups.iter().any(|g| g.id == gid) {
                return Err(format!("no group {gid}"));
            }
        }
        for g in self.groups.iter_mut() {
            g.projects.retain(|p| p != project_id);
        }
        if let Some(gid) = group_id {
            if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) {
                g.projects.push(project_id.to_string());
            }
        }
        self.broadcast_event(
            "group.updated",
            json!({"project_id": project_id, "group_id": group_id}),
        );
        Ok(())
    }

    pub fn terminal_json(&self, id: &str) -> Value {
        match self.terminals.get(id) {
            Some(t) => {
                let t = lock(t);
                let (off, max) = t.scroll_metrics();
                json!({
                    "term_id": t.id,
                    "cwd": t.cwd,
                    "command": t.command,
                    "label": t.label,
                    "kind": t.kind,
                    "branch": t.branch,
                    "cols": t.cols(),
                    "rows": t.rows(),
                    "dead": t.is_dead(),
                    "agent_kind": t.agent_kind,
                    "agent_status": t.agent_status,
                    "scroll": {
                        "offset_from_bottom": off,
                        "max_offset_from_bottom": max,
                        "viewport_rows": t.rows(),
                    },
                })
            }
            None => Value::Null,
        }
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "focused_project_id": self.focused_project_id,
            "groups": self.groups.iter().map(|g| self.group_json(g)).collect::<Vec<_>>(),
            "projects": self.projects.keys().map(|id| self.project_json(id)).collect::<Vec<_>>(),
            "terminals": self.terminals.keys().map(|id| self.terminal_json(id)).collect::<Vec<_>>(),
        })
    }

    pub fn close_terminal(&mut self, id: &str) -> Result<(), String> {
        let t = self
            .terminals
            .remove(id)
            .ok_or_else(|| format!("no terminal {id}"))?;
        lock(&t).kill();
        for p in self.projects.values_mut() {
            p.terminals.retain(|x| x != id);
        }
        self.broadcast_event("terminal.closed", json!({"term_id": id}));
        Ok(())
    }

    pub fn close_project(&mut self, id: &str) -> Result<(), String> {
        let p = self
            .projects
            .remove(id)
            .ok_or_else(|| format!("no project {id}"))?;
        for g in self.groups.iter_mut() {
            g.projects.retain(|x| x != id);
        }
        for tid in p.terminals {
            if let Some(t) = self.terminals.remove(&tid) {
                lock(&t).kill();
                self.broadcast_event("terminal.closed", json!({"term_id": tid}));
            }
        }
        if self.focused_project_id.as_deref() == Some(id) {
            self.focused_project_id = None;
        }
        self.broadcast_event("project.closed", json!({"project_id": id}));
        Ok(())
    }

    pub fn get_terminal(&self, id: &str) -> Result<Arc<Mutex<PtyTerm>>, String> {
        self.terminals
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no terminal {id}"))
    }

    pub fn broadcast_event(&mut self, name: &str, data: Value) {
        let msg = json!({"event": name, "data": data});
        self.event_subs.retain(|s| s.send(msg.clone()).is_ok());
    }

    /// Agent watch thread — re-detects each terminal's agent on a tick and
    /// pushes `agent.status` events to global subscribers on change. The
    /// session lock is never held across a terminal lock, so one stalled
    /// terminal can't freeze the whole session.
    pub fn agent_watch(session: Arc<Mutex<Session>>) {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let terms: Vec<(String, Arc<Mutex<PtyTerm>>)> = lock(&session)
                .terminals
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (tid, t) in terms {
                let changed = lock(&t).detect_agent();
                if changed {
                    let mut s = lock(&session);
                    let j = s.terminal_json(&tid);
                    s.broadcast_event(
                        "agent.status",
                        json!({
                            "term_id": tid,
                            "agent": j.get("agent_kind"),
                            "agent_status": j.get("agent_status"),
                        }),
                    );
                }
            }
        }
    }

    /// Persist the session: model + each terminal's raw output tail.
    pub fn persist(&self) {
        let terminals: Vec<PersistedTerm> = self
            .terminals
            .values()
            .map(|t| {
                let t = lock(t);
                PersistedTerm {
                    id: t.id.clone(),
                    project_id: self
                        .projects
                        .iter()
                        .find(|(_, p)| p.terminals.contains(&t.id))
                        .map(|(pid, _)| pid.clone())
                        .unwrap_or_default(),
                    cwd: t.cwd.clone(),
                    command: t.command.clone(),
                    label: t.label.clone(),
                    kind: t.kind.clone(),
                    cols: t.cols(),
                    rows: t.rows(),
                    tail_b64: base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        t.tail_bytes(),
                    ),
                }
            })
            .collect();
        let p = Persisted {
            version: 1,
            next_term: self.next_term,
            next_project: self.next_project,
            next_group: self.next_group,
            focused_project_id: self.focused_project_id.clone(),
            projects: self.projects.values().cloned().collect(),
            groups: self.groups.clone(),
            terminals,
        };
        if let Ok(s) = serde_json::to_string_pretty(&p) {
            let _ = std::fs::write(state::session_path(), s);
        }
    }

    /// Restore a persisted session — recreates projects and terminals,
    /// replaying each terminal's output tail into the screen model.
    pub fn restore(&mut self) {
        let Ok(raw) = std::fs::read_to_string(state::session_path()) else {
            return;
        };
        let Ok(p) = serde_json::from_str::<Persisted>(&raw) else {
            return;
        };
        self.next_term = p.next_term.max(self.next_term);
        self.next_project = p.next_project.max(self.next_project);
        self.next_group = p.next_group.max(self.next_group);
        self.focused_project_id = p.focused_project_id;
        for proj in p.projects {
            self.projects.insert(proj.id.clone(), proj);
        }
        self.groups = p.groups;
        // drop memberships pointing at projects that no longer exist
        for g in self.groups.iter_mut() {
            g.projects.retain(|pid| self.projects.contains_key(pid));
        }
        for pt in p.terminals {
            // sessions persisted before the commander concept was dropped
            // restore those terminals as plain ones
            let kind = if pt.kind == "commander" {
                "plain".to_string()
            } else {
                pt.kind.clone()
            };
            let label = if pt.label.as_deref() == Some("commander") {
                None
            } else {
                pt.label.clone()
            };
            let (term, mut reader) = match PtyTerm::spawn(
                &pt.id, &pt.cwd, &pt.command, label, &kind, pt.cols, pt.rows,
            ) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let tail = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &pt.tail_b64,
            )
            .unwrap_or_default();
            let mut term = term;
            term.replay(&tail);
            let term = Arc::new(Mutex::new(term));
            {
                let t = term.clone();
                std::thread::spawn(move || PtyTerm::pump(&t, reader.as_mut()));
            }
            {
                let t = term.clone();
                std::thread::spawn(move || PtyTerm::render_loop(&t));
            }
            self.terminals.insert(pt.id.clone(), term);
            if let Some(proj) = self.projects.get_mut(&pt.project_id) {
                if !proj.terminals.contains(&pt.id) {
                    proj.terminals.push(pt.id);
                }
            }
        }
    }
}
