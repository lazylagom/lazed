//! Organization overlay on herdr: session > group? > project > herdr workspace.
//!
//! herdr owns workspace / tab / pane (and their ids — `w1`, `w1:t1`,
//! `w1:p1`). lazed adds only what herdr has no notion of:
//!
//! - group:   an optional named, ordered collection of projects (sidebar
//!            sections) — lazed id `gN`
//! - project: one git repo, identified by its shared git-common-dir
//!            (`repo_key`) — lazed id `pN`. Owns the ordered list of herdr
//!            workspace ids checked out from that repo; index 0 is the
//!            main checkout.
//! - workspace annotation: for a herdr workspace id, which project it
//!            belongs to, its checkout path/branch, and whether it is the
//!            main checkout or a linked worktree.
//!
//! This module is a pure model: no herdr IO, no git IO except
//! `resolve_repo`/`detect_branch` helpers the server calls *outside* the
//! session lock. `snapshot_json` merges a herdr snapshot the caller
//! fetched with the annotations here; `reconcile` keeps the annotations
//! honest against that snapshot (workspaces closed from the herdr TUI
//! disappear here too, workspaces opened there get adopted).
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::server::ClientSender;
use crate::state;

/// Lock a mutex, recovering from poison — a panic in one thread must not
/// wedge every thread that later touches the same mutex.
pub fn lock<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| {
        eprintln!("lazed: recovered poisoned mutex");
        e.into_inner()
    })
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub label: Option<String>,
    pub repo_root: String,
    pub repo_key: String,
    /// ordered herdr workspace ids — index 0 is the main-checkout workspace
    pub workspaces: Vec<String>,
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

/// lazed's annotation of one herdr workspace: which checkout of which
/// project it is. `id` is the herdr workspace id.
#[derive(Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub project_id: String,
    pub label: Option<String>,
    /// checkout path (repo_root for the main workspace)
    pub path: String,
    /// branch at open — the card's display name for worktrees
    pub branch: Option<String>,
    pub is_main: bool,
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    version: u32,
    next_project: u64,
    next_group: u64,
    focused_project_id: Option<String>,
    projects: Vec<Project>,
    groups: Vec<Group>,
    workspaces: Vec<Workspace>,
}

/// v3 = herdr overlay. v2 files (own PTY terminals) carry nothing worth
/// migrating — their panes are gone with the old daemon — so a foreign
/// version just starts a fresh session.
const SESSION_VERSION: u32 = 3;

pub struct Session {
    pub projects: HashMap<String, Project>,
    /// herdr workspace id → annotation
    pub workspaces: HashMap<String, Workspace>,
    /// ordered groups — display order in the sidebar
    pub groups: Vec<Group>,
    pub removing_workspaces: std::collections::HashSet<String>,
    next_project: u64,
    next_group: u64,
    pub focused_project_id: Option<String>,
    /// global event subscribers (events.subscribe)
    pub event_subs: Vec<ClientSender>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Session {
            projects: HashMap::new(),
            workspaces: HashMap::new(),
            groups: Vec::new(),
            removing_workspaces: Default::default(),
            next_project: 1,
            next_group: 1,
            focused_project_id: None,
            event_subs: Vec::new(),
        }
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
    /// for non-repo dirs. Spawns git — call outside the session lock.
    pub fn resolve_repo(cwd: &str) -> (String, String) {
        let root = git_out(cwd, &["rev-parse", "--show-toplevel"]).unwrap_or_else(|| cwd.to_string());
        let key = git_out(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .unwrap_or_else(|| root.clone());
        (root, key)
    }

    /// Current branch of a checkout (None when detached / not a repo).
    pub fn detect_branch(cwd: &str) -> Option<String> {
        git_out(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD")
    }

    // ── projects ──────────────────────────────────────────────────────────

    pub fn project_by_repo_key(&self, repo_key: &str) -> Option<&Project> {
        self.projects.values().find(|p| p.repo_key == repo_key)
    }

    /// Register a project for a resolved repo (no herdr IO). Returns the
    /// existing project when the repo is already known.
    pub fn ensure_project(
        &mut self,
        repo_root: &str,
        repo_key: &str,
        label: Option<String>,
        group_id: Option<&str>,
    ) -> Result<String, String> {
        if let Some(p) = self.project_by_repo_key(repo_key) {
            return Ok(p.id.clone());
        }
        if let Some(gid) = group_id {
            if !self.groups.iter().any(|g| g.id == gid) {
                return Err(format!("no group {gid}"));
            }
        }
        let id = self.alloc_project();
        self.projects.insert(
            id.clone(),
            Project {
                id: id.clone(),
                label,
                repo_root: repo_root.to_string(),
                repo_key: repo_key.to_string(),
                workspaces: Vec::new(),
            },
        );
        if let Some(gid) = group_id {
            if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) {
                g.projects.push(id.clone());
            }
        }
        self.broadcast_event("project.created", json!({"project_id": id}));
        Ok(id)
    }

    /// The project's main-checkout workspace (workspaces[0] by
    /// construction, but find it by flag for safety).
    pub fn main_workspace(&self, project_id: &str) -> Option<&Workspace> {
        self.projects.get(project_id).and_then(|p| {
            p.workspaces
                .iter()
                .filter_map(|id| self.workspaces.get(id))
                .find(|w| w.is_main)
                .or_else(|| p.workspaces.first().and_then(|id| self.workspaces.get(id)))
        })
    }

    /// Drop a project and its workspace annotations. The caller closes the
    /// herdr workspaces (or leaves them, when herdr is down).
    pub fn close_project(&mut self, id: &str) -> Result<Vec<String>, String> {
        let p = self
            .projects
            .remove(id)
            .ok_or_else(|| format!("no project {id}"))?;
        for g in self.groups.iter_mut() {
            g.projects.retain(|x| x != id);
        }
        for wid in &p.workspaces {
            self.workspaces.remove(wid);
            self.broadcast_event(
                "workspace.removed",
                json!({"workspace_id": wid, "project_id": id}),
            );
        }
        if self.focused_project_id.as_deref() == Some(id) {
            self.focused_project_id = None;
        }
        self.broadcast_event("project.closed", json!({"project_id": id}));
        Ok(p.workspaces)
    }

    // ── workspaces (herdr ids) ────────────────────────────────────────────

    /// Annotate a herdr workspace as a checkout of `project_id`. The main
    /// checkout is kept at index 0 of the project's list.
    pub fn register_workspace(
        &mut self,
        herdr_workspace_id: &str,
        project_id: &str,
        path: &str,
        label: Option<String>,
        branch: Option<String>,
        is_main: bool,
    ) -> Result<Value, String> {
        if !self.projects.contains_key(project_id) {
            return Err(format!("no project {project_id}"));
        }
        let id = herdr_workspace_id.to_string();
        self.workspaces.insert(
            id.clone(),
            Workspace {
                id: id.clone(),
                project_id: project_id.to_string(),
                label,
                path: path.to_string(),
                branch,
                is_main,
            },
        );
        if let Some(p) = self.projects.get_mut(project_id) {
            p.workspaces.retain(|w| w != &id);
            if is_main {
                p.workspaces.insert(0, id.clone());
            } else {
                p.workspaces.push(id.clone());
            }
        }
        self.broadcast_event(
            "workspace.created",
            json!({"workspace_id": id, "project_id": project_id, "path": path}),
        );
        Ok(self.workspace_json(&id))
    }

    /// Forget a herdr workspace. The caller decides about the herdr side
    /// (`workspace.close`) and the checkout on disk.
    pub fn close_workspace(&mut self, id: &str) -> Result<(), String> {
        let ws = self
            .workspaces
            .remove(id)
            .ok_or_else(|| format!("no workspace {id}"))?;
        for p in self.projects.values_mut() {
            p.workspaces.retain(|x| x != id);
        }
        self.broadcast_event(
            "workspace.removed",
            json!({"workspace_id": id, "project_id": ws.project_id}),
        );
        Ok(())
    }

    // ── reconcile against herdr ───────────────────────────────────────────

    /// Drop annotations for workspaces herdr no longer has (closed from the
    /// TUI, server restarted without them). Returns the herdr workspaces
    /// lazed has no annotation for, with the cwd of one of their panes —
    /// the caller resolves the repo outside the lock and `adopt`s them.
    pub fn reconcile(&mut self, herdr: &Value) -> Vec<(String, String)> {
        let live: HashMap<String, &Value> = herdr
            .get("workspaces")
            .and_then(Value::as_array)
            .map(|ws| {
                ws.iter()
                    .filter_map(|w| w.get("workspace_id").and_then(Value::as_str).map(|id| (id.to_string(), w)))
                    .collect()
            })
            .unwrap_or_default();
        let gone: Vec<String> = self
            .workspaces
            .keys()
            .filter(|id| !live.contains_key(*id) && !self.removing_workspaces.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            let _ = self.close_workspace(&id);
        }
        let mut unknown = Vec::new();
        for id in live.keys() {
            if self.workspaces.contains_key(id) {
                continue;
            }
            let cwd = herdr
                .get("panes")
                .and_then(Value::as_array)
                .and_then(|ps| {
                    ps.iter()
                        .filter(|p| p.get("workspace_id").and_then(Value::as_str) == Some(id))
                        .find_map(|p| p.get("cwd").and_then(Value::as_str))
                })
                .map(str::to_string);
            if let Some(cwd) = cwd {
                unknown.push((id.clone(), cwd));
            }
        }
        unknown
    }

    /// Adopt a herdr workspace lazed didn't open: file it under the repo's
    /// project (creating the project when needed). `is_main` when the
    /// checkout is the repo root itself.
    pub fn adopt_workspace(
        &mut self,
        herdr_workspace_id: &str,
        cwd: &str,
        repo_root: &str,
        repo_key: &str,
        branch: Option<String>,
    ) -> Result<Value, String> {
        let project_id = self.ensure_project(repo_root, repo_key, None, None)?;
        let is_main = same_path(cwd, repo_root)
            && !self
                .projects
                .get(&project_id)
                .map(|p| p.workspaces.iter().any(|w| self.workspaces.get(w).is_some_and(|x| x.is_main)))
                .unwrap_or(false);
        self.register_workspace(herdr_workspace_id, &project_id, cwd, None, branch, is_main)
    }

    // ── json ──────────────────────────────────────────────────────────────

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
                "workspaces": p.workspaces,
                "focused": self.focused_project_id.as_deref() == Some(id),
            }),
            None => Value::Null,
        }
    }

    /// Annotation only — `snapshot_json` merges herdr's own WorkspaceInfo in.
    pub fn workspace_json(&self, id: &str) -> Value {
        match self.workspaces.get(id) {
            Some(w) => json!({
                "workspace_id": w.id,
                "project_id": w.project_id,
                "label": w.label,
                "path": w.path,
                "branch": w.branch,
                "is_main": w.is_main,
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

    /// The client snapshot: lazed's groups/projects + herdr's
    /// workspaces/tabs/panes/agents/layouts, each workspace carrying its
    /// lazed annotation. `herdr` is None while herdr is unreachable — the
    /// organization layer still renders (degraded mode), with the
    /// annotated workspaces and no tabs/panes.
    pub fn snapshot_json(&self, herdr: Option<&Value>) -> Value {
        let annotate = |w: &Value| -> Value {
            let mut w = w.clone();
            let id = w.get("workspace_id").and_then(Value::as_str).unwrap_or("");
            match self.workspaces.get(id) {
                Some(a) => {
                    w["project_id"] = json!(a.project_id);
                    w["path"] = json!(a.path);
                    w["branch"] = json!(a.branch);
                    w["is_main"] = json!(a.is_main);
                    if let Some(l) = &a.label {
                        w["label"] = json!(l);
                    }
                }
                None => {
                    w["project_id"] = Value::Null;
                    w["is_main"] = json!(false);
                }
            }
            w
        };
        let herdr_arr = |k: &str| -> Vec<Value> {
            herdr
                .and_then(|h| h.get(k))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let workspaces: Vec<Value> = match herdr {
            Some(h) => h
                .get("workspaces")
                .and_then(Value::as_array)
                .map(|ws| ws.iter().map(annotate).collect())
                .unwrap_or_default(),
            None => self.workspaces.keys().map(|id| self.workspace_json(id)).collect(),
        };
        json!({
            "focused_project_id": self.focused_project_id,
            "groups": self.groups.iter().map(|g| self.group_json(g)).collect::<Vec<_>>(),
            "projects": self.projects.keys().map(|id| self.project_json(id)).collect::<Vec<_>>(),
            "workspaces": workspaces,
            "tabs": herdr_arr("tabs"),
            "panes": herdr_arr("panes"),
            "agents": herdr_arr("agents"),
            "layouts": herdr_arr("layouts"),
            "focused_workspace_id": herdr.and_then(|h| h.get("focused_workspace_id")).cloned().unwrap_or(Value::Null),
            "focused_tab_id": herdr.and_then(|h| h.get("focused_tab_id")).cloned().unwrap_or(Value::Null),
            "focused_pane_id": herdr.and_then(|h| h.get("focused_pane_id")).cloned().unwrap_or(Value::Null),
            "herdr": {
                "connected": herdr.is_some(),
                "session": crate::herdr::session_name(),
                "version": herdr.and_then(|h| h.get("version")).cloned().unwrap_or(Value::Null),
            },
        })
    }

    // ── groups ────────────────────────────────────────────────────────────

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

    // ── events / persistence ──────────────────────────────────────────────

    pub fn broadcast_event(&mut self, name: &str, data: Value) {
        let msg = json!({"event": name, "data": data});
        self.event_subs.retain(|s| s.send(msg.clone()).is_ok());
    }

    pub fn persist(&self) -> Result<(), String> {
        let p = Persisted {
            version: SESSION_VERSION,
            next_project: self.next_project,
            next_group: self.next_group,
            focused_project_id: self.focused_project_id.clone(),
            projects: self.projects.values().cloned().collect(),
            groups: self.groups.clone(),
            workspaces: self.workspaces.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&p).map_err(|e| e.to_string())?;
        state::atomic_write(&state::session_path(), &bytes)
            .map_err(|e| format!("session persist failed: {e}"))
    }

    /// Restore annotations. herdr restores the panes themselves; the first
    /// `reconcile` after boot drops annotations for workspaces herdr lost.
    pub fn restore(&mut self) {
        let Ok(raw) = std::fs::read_to_string(state::session_path()) else {
            return;
        };
        let Ok(p) = serde_json::from_str::<Persisted>(&raw) else {
            return;
        };
        if p.version != SESSION_VERSION {
            return;
        }
        self.next_project = p.next_project.max(self.next_project);
        self.next_group = p.next_group.max(self.next_group);
        self.focused_project_id = p.focused_project_id;
        for proj in p.projects {
            self.projects.insert(proj.id.clone(), proj);
        }
        self.groups = p.groups;
        for ws in p.workspaces {
            self.workspaces.insert(ws.id.clone(), ws);
        }
        // drop memberships pointing at objects that no longer exist
        for g in self.groups.iter_mut() {
            g.projects.retain(|pid| self.projects.contains_key(pid));
        }
        for proj in self.projects.values_mut() {
            proj.workspaces.retain(|wid| self.workspaces.contains_key(wid));
        }
        self.workspaces.retain(|_, w| self.projects.contains_key(&w.project_id));
    }
}

fn git_out(cwd: &str, args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn same_path(a: &str, b: &str) -> bool {
    let canon = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| std::path::PathBuf::from(p));
    canon(a) == canon(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_project() -> (Session, String) {
        let mut s = Session::new();
        let pid = s.ensure_project("/repo", "/repo/.git", None, None).unwrap();
        (s, pid)
    }

    #[test]
    fn ensure_project_dedupes_by_repo_key() {
        let (mut s, pid) = with_project();
        assert_eq!(s.ensure_project("/repo/sub", "/repo/.git", None, None).unwrap(), pid);
        assert_eq!(s.projects.len(), 1);
    }

    #[test]
    fn main_workspace_sorts_first() {
        let (mut s, pid) = with_project();
        s.register_workspace("w2", &pid, "/wt/fix", None, Some("fix".into()), false).unwrap();
        s.register_workspace("w1", &pid, "/repo", None, Some("main".into()), true).unwrap();
        assert_eq!(s.projects[&pid].workspaces, vec!["w1", "w2"]);
        assert_eq!(s.main_workspace(&pid).unwrap().id, "w1");
    }

    #[test]
    fn reconcile_drops_gone_and_reports_unknown() {
        let (mut s, pid) = with_project();
        s.register_workspace("w1", &pid, "/repo", None, None, true).unwrap();
        let herdr = json!({
            "workspaces": [{"workspace_id": "w3"}],
            "panes": [{"pane_id": "w3:p1", "workspace_id": "w3", "cwd": "/other"}],
        });
        let unknown = s.reconcile(&herdr);
        assert!(!s.workspaces.contains_key("w1"));
        assert!(s.projects[&pid].workspaces.is_empty());
        assert_eq!(unknown, vec![("w3".to_string(), "/other".to_string())]);
    }

    #[test]
    fn reconcile_keeps_workspaces_mid_removal() {
        let (mut s, pid) = with_project();
        s.register_workspace("w1", &pid, "/repo", None, None, true).unwrap();
        s.removing_workspaces.insert("w1".into());
        s.reconcile(&json!({"workspaces": [], "panes": []}));
        assert!(s.workspaces.contains_key("w1"));
    }

    #[test]
    fn adopt_files_under_repo_project() {
        let mut s = Session::new();
        s.adopt_workspace("w1", "/tmp", "/tmp", "/tmp", None).unwrap();
        let pid = s.workspaces["w1"].project_id.clone();
        assert!(s.workspaces["w1"].is_main);
        // second checkout of the same repo is not main
        s.adopt_workspace("w2", "/tmp", "/tmp", "/tmp", Some("x".into())).unwrap();
        assert_eq!(s.workspaces["w2"].project_id, pid);
        assert!(!s.workspaces["w2"].is_main);
    }

    #[test]
    fn snapshot_merges_annotations_and_degrades() {
        let (mut s, pid) = with_project();
        s.register_workspace("w1", &pid, "/repo", Some("Repo".into()), Some("main".into()), true).unwrap();
        let herdr = json!({
            "version": "0.9.0",
            "workspaces": [{"workspace_id": "w1", "label": "repo", "pane_count": 1}, {"workspace_id": "w9", "label": "x"}],
            "tabs": [{"tab_id": "w1:t1"}], "panes": [], "agents": [], "layouts": [],
        });
        let snap = s.snapshot_json(Some(&herdr));
        let ws = snap["workspaces"].as_array().unwrap();
        assert_eq!(ws[0]["project_id"], pid);
        assert_eq!(ws[0]["label"], "Repo");
        assert_eq!(ws[0]["pane_count"], 1);
        assert!(ws[1]["project_id"].is_null());
        assert_eq!(snap["herdr"]["connected"], true);
        let degraded = s.snapshot_json(None);
        assert_eq!(degraded["herdr"]["connected"], false);
        assert_eq!(degraded["workspaces"][0]["workspace_id"], "w1");
        assert!(degraded["tabs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn close_project_returns_workspaces_to_close() {
        let (mut s, pid) = with_project();
        s.register_workspace("w1", &pid, "/repo", None, None, true).unwrap();
        s.register_workspace("w2", &pid, "/wt", None, None, false).unwrap();
        let ws = s.close_project(&pid).unwrap();
        assert_eq!(ws, vec!["w1", "w2"]);
        assert!(s.workspaces.is_empty());
    }
}
