//! Domain model: session > group? > project > workspace > tab > pane.
//!
//! - session: the running daemon's namespace (persisted to session.json)
//! - group: an optional named collection of projects (sidebar sections)
//! - project: a repo — owns workspaces, knows repo_root/repo_key
//! - workspace: one checkout — the main checkout (is_main) or a linked
//!   git worktree. Owns ordered tabs.
//! - tab: a named leaf container inside a workspace — its panes render
//!   as a split row in the workspace view
//! - pane: one PTY (terminals map — `tN` ids, PtyTerm machinery)
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use crate::server::ClientSender;
use std::sync::{Arc, Mutex};

use crate::state;
use crate::term::{lock, PtyTerm};

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub label: Option<String>,
    pub repo_root: String,
    pub repo_key: String,
    /// ordered workspace ids — index 0 is the main-checkout workspace
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

/// One checkout of the project's repo — the main checkout or a linked
/// git worktree. This is the sidebar's "workspace card".
#[derive(Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub project_id: String,
    pub label: Option<String>,
    /// checkout path (repo_root for the main workspace)
    pub path: String,
    /// branch at spawn — the card's display name
    pub branch: Option<String>,
    pub is_main: bool,
    /// ordered tab ids
    pub tabs: Vec<String>,
}

/// A tab inside a workspace — an ordered row of panes.
#[derive(Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: String,
    pub workspace_id: String,
    pub label: Option<String>,
    /// ordered pane (terminal) ids
    pub panes: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct PersistedTerm {
    id: String,
    tab_id: String,
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
    next_group: u64,
    next_workspace: u64,
    next_tab: u64,
    focused_project_id: Option<String>,
    projects: Vec<Project>,
    groups: Vec<Group>,
    workspaces: Vec<Workspace>,
    tabs: Vec<Tab>,
    terminals: Vec<PersistedTerm>,
}

/// Current session file format — v1 (project > terminal) files are not
/// read at all; a failed/foreign parse just starts a fresh session.
const SESSION_VERSION: u32 = 2;

pub struct Session {
    // Live names never survive daemon restart or identify restored shells.
    pub agent_names: HashMap<String, crate::named::Binding>,
    pub removing_workspaces: std::collections::HashSet<String>,
    pub projects: HashMap<String, Project>,
    pub workspaces: HashMap<String, Workspace>,
    pub tabs: HashMap<String, Tab>,
    pub terminals: HashMap<String, Arc<Mutex<PtyTerm>>>,
    /// ordered groups — display order in the sidebar
    pub groups: Vec<Group>,
    next_term: u64,
    next_project: u64,
    next_group: u64,
    next_workspace: u64,
    next_tab: u64,
    pub focused_project_id: Option<String>,
    /// global event subscribers (events.subscribe)
    pub event_subs: Vec<ClientSender>,
}

impl Session {
    pub fn new() -> Self {
        Session {
            agent_names: HashMap::new(),
            removing_workspaces: Default::default(),
            projects: HashMap::new(),
            workspaces: HashMap::new(),
            tabs: HashMap::new(),
            terminals: HashMap::new(),
            groups: Vec::new(),
            next_term: 1,
            next_project: 1,
            next_group: 1,
            next_workspace: 1,
            next_tab: 1,
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

    fn alloc_workspace(&mut self) -> String {
        let id = format!("w{}", self.next_workspace);
        self.next_workspace += 1;
        id
    }

    fn alloc_tab(&mut self) -> String {
        let id = format!("tb{}", self.next_tab);
        self.next_tab += 1;
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

    /// Spawn one pane (PTY) and append it to a tab's pane row.
    pub fn create_terminal(
        &mut self,
        tab_id: &str,
        cwd: &str,
        command: &str,
        label: Option<String>,
        kind: &str,
        cols: usize,
        rows: usize,
    ) -> Result<Arc<Mutex<PtyTerm>>, String> {
        let tab = self
            .tabs
            .get(tab_id)
            .ok_or_else(|| format!("no tab {tab_id}"))?;
        let workspace_id = tab.workspace_id.clone();
        if self.removing_workspaces.contains(&workspace_id) { return Err("workspace removal in progress".into()); }
        let project_id = self
            .workspaces
            .get(&workspace_id)
            .map(|w| w.project_id.clone())
            .unwrap_or_default();
        let id = self.alloc_term();
        let (term, mut reader) = PtyTerm::spawn(
            &id, cwd, command, label, kind, cols, rows, &workspace_id,
        )
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
        if let Some(tab) = self.tabs.get_mut(tab_id) {
            tab.panes.push(id.clone());
        }
        self.broadcast_event(
            "terminal.created",
            json!({
                "term_id": id,
                "tab_id": tab_id,
                "workspace_id": workspace_id,
                "project_id": project_id,
                "cwd": cwd,
            }),
        );
        Ok(term)
    }

    /// A new tab inside a workspace — always opens with one pane.
    pub fn create_tab(
        &mut self,
        workspace_id: &str,
        label: Option<String>,
        command: &str,
    ) -> Result<Value, String> {
        if self.removing_workspaces.contains(workspace_id) { return Err("workspace removal in progress".into()); }
        let ws = self
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| format!("no workspace {workspace_id}"))?;
        let path = ws.path.clone();
        let kind = if ws.is_main { "plain" } else { "worktree" };
        let id = self.create_empty_tab(workspace_id, label);
        let term = match self.create_terminal(&id, &path, command, None, kind, 80, 24) {
            Ok(term) => term,
            Err(error) => {
                self.remove_tab(&id);
                return Err(error);
            }
        };
        let tid = lock(&term).id.clone();
        Ok(json!({
            "tab_id": id,
            "terminal": self.terminal_json(&tid),
        }))
    }

    /// Keep tab identity independent of the lifetime of its shell.
    fn create_empty_tab(&mut self, workspace_id: &str, label: Option<String>) -> String {
        let id = self.alloc_tab();
        self.tabs.insert(
            id.clone(),
            Tab {
                id: id.clone(),
                workspace_id: workspace_id.to_string(),
                label,
                panes: Vec::new(),
            },
        );
        if let Some(ws) = self.workspaces.get_mut(workspace_id) {
            ws.tabs.push(id.clone());
        }
        self.broadcast_event(
            "tab.created",
            json!({"tab_id": id, "workspace_id": workspace_id}),
        );
        id
    }

    fn is_last_tab(&self, id: &str) -> bool {
        self.tabs.get(id)
            .and_then(|tab| self.workspaces.get(&tab.workspace_id))
            .map(|ws| ws.tabs.len() == 1 && ws.tabs[0] == id)
            .unwrap_or(false)
    }

    /// Repair older sessions while retaining a usable tab if shell startup fails.
    fn ensure_default_tabs(&mut self) {
        let empty: Vec<_> = self.workspaces.values()
            .filter(|ws| ws.tabs.is_empty())
            .map(|ws| (ws.id.clone(), ws.path.clone(), ws.is_main))
            .collect();
        for (workspace_id, path, is_main) in empty {
            let id = self.create_empty_tab(&workspace_id, None);
            let kind = if is_main { "plain" } else { "worktree" };
            if let Err(error) = self.create_terminal(&id, &path, "", None, kind, 80, 24) {
                eprintln!("default terminal for {workspace_id}: {error}");
            }
        }
    }

    /// A new workspace under a project — one checkout (main or worktree)
    /// plus its first tab and pane.
    pub fn create_workspace(
        &mut self,
        project_id: &str,
        path: &str,
        label: Option<String>,
        branch: Option<String>,
        is_main: bool,
    ) -> Result<Value, String> {
        if !self.projects.contains_key(project_id) {
            return Err(format!("no project {project_id}"));
        }
        let id = self.alloc_workspace();
        self.workspaces.insert(
            id.clone(),
            Workspace {
                id: id.clone(),
                project_id: project_id.to_string(),
                label,
                path: path.to_string(),
                branch,
                is_main,
                tabs: Vec::new(),
            },
        );
        if let Some(p) = self.projects.get_mut(project_id) {
            p.workspaces.push(id.clone());
        }
        self.broadcast_event(
            "workspace.created",
            json!({"workspace_id": id, "project_id": project_id, "path": path}),
        );
        let tab = match self.create_tab(&id, None, "") {
            Ok(tab) => tab,
            Err(error) => {
                let _ = self.close_workspace(&id);
                return Err(error);
            }
        };
        Ok(json!({
            "workspace": self.workspace_json(&id),
            "tab_id": tab.get("tab_id"),
            "terminal": tab.get("terminal"),
        }))
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
                repo_root: repo_root.clone(),
                repo_key,
                workspaces: Vec::new(),
            },
        );
        if let Some(gid) = group_id {
            if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) {
                g.projects.push(id.clone());
            }
        }
        // the main workspace = the repo's root checkout
        let branch = crate::term::detect_branch(&repo_root);
        let ws = match self.create_workspace(&id, &repo_root, None, branch, true) {
            Ok(ws) => ws,
            Err(error) => {
                let _ = self.close_project(&id);
                return Err(error);
            }
        };
        self.broadcast_event("project.created", json!({"project_id": id}));
        Ok(json!({
            "project": self.project_json(&id),
            "workspace": ws.get("workspace"),
            "terminal": ws.get("terminal"),
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
                "workspaces": p.workspaces,
                "focused": self.focused_project_id.as_deref() == Some(id),
            }),
            None => Value::Null,
        }
    }

    pub fn workspace_json(&self, id: &str) -> Value {
        match self.workspaces.get(id) {
            Some(w) => json!({
                "workspace_id": w.id,
                "project_id": w.project_id,
                "label": w.label,
                "path": w.path,
                "branch": w.branch,
                "is_main": w.is_main,
                "tabs": w.tabs,
            }),
            None => Value::Null,
        }
    }

    pub fn tab_json(&self, id: &str) -> Value {
        match self.tabs.get(id) {
            Some(t) => json!({
                "tab_id": t.id,
                "workspace_id": t.workspace_id,
                "label": t.label,
                "panes": t.panes,
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

    /// A pane's ancestry: (tab, workspace, project) ids for events/json.
    pub(crate) fn term_parents(&self, term_id: &str) -> (Option<String>, Option<String>, Option<String>) {
        let tab = self
            .tabs
            .values()
            .find(|t| t.panes.iter().any(|p| p == term_id));
        match tab {
            Some(t) => {
                let ws = self.workspaces.get(&t.workspace_id);
                (
                    Some(t.id.clone()),
                    Some(t.workspace_id.clone()),
                    ws.map(|w| w.project_id.clone()),
                )
            }
            None => (None, None, None),
        }
    }

    pub fn terminal_json(&self, id: &str) -> Value {
        match self.terminals.get(id) {
            Some(t) => {
                let t = lock(t);
                let (off, max, _, _) = t.scroll_metrics();
                let (tab_id, workspace_id, project_id) = self.term_parents(id);
                json!({
                    "term_id": t.id,
                    "tab_id": tab_id,
                    "workspace_id": workspace_id,
                    "project_id": project_id,
                    "cwd": t.cwd,
                    "command": t.command,
                    "label": t.label,
                    "kind": t.kind,
                    "branch": t.branch,
                    "cols": t.cols(),
                    "rows": t.rows(),
                    "dead": t.is_dead(),
                    "agent_kind": t.agent_kind,
                    "agent_name": self.agent_names.iter()
                        .find(|(_, binding)| binding.term_id == t.id && t.lifecycle.launch_id.as_deref() == Some(binding.launch_id.as_str()))
                        .map(|(name, _)| name),
                    "agent_status": t.agent_status,
                    "launch_id": t.lifecycle.launch_id,
                    "state_seq": t.lifecycle.state_seq,
                    "status_source": t.lifecycle.source,
                    "scroll": {
                        "offset_from_bottom": off,
                        "max_offset_from_bottom": max,
                        "viewport_cols": t.cols(),
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
            "workspaces": self.workspaces.keys().map(|id| self.workspace_json(id)).collect::<Vec<_>>(),
            "tabs": self.tabs.keys().map(|id| self.tab_json(id)).collect::<Vec<_>>(),
            "terminals": self.terminals.keys().map(|id| self.terminal_json(id)).collect::<Vec<_>>(),
        })
    }

    /// Close one pane. Remove empty extra tabs, retaining the workspace's last tab.
    pub fn close_terminal(&mut self, id: &str) -> Result<(), String> {
        let t = self
            .terminals
            .remove(id)
            .ok_or_else(|| format!("no terminal {id}"))?;
        lock(&t).kill();
        let ws_id = self.workspace_of_term(id).map(|w| w.id.clone());
        let mut empty_tab: Option<String> = None;
        for tab in self.tabs.values_mut() {
            if tab.panes.iter().any(|p| p == id) {
                tab.panes.retain(|x| x != id);
                if tab.panes.is_empty() {
                    empty_tab = Some(tab.id.clone());
                }
            }
        }
        self.broadcast_event(
            "terminal.closed",
            json!({"term_id": id, "workspace_id": ws_id}),
        );
        if let Some(tid) = empty_tab {
            if !self.is_last_tab(&tid) {
                self.remove_tab(&tid);
            }
        }
        Ok(())
    }

    /// Remove a tab and kill its panes. Internal — callers emit nothing.
    fn remove_tab(&mut self, id: &str) {
        let Some(tab) = self.tabs.remove(id) else { return };
        for pid in tab.panes {
            if let Some(t) = self.terminals.remove(&pid) {
                lock(&t).kill();
                self.broadcast_event("terminal.closed", json!({"term_id": pid}));
            }
        }
        for ws in self.workspaces.values_mut() {
            ws.tabs.retain(|x| x != id);
        }
        self.broadcast_event(
            "tab.closed",
            json!({"tab_id": id, "workspace_id": tab.workspace_id}),
        );
    }

    /// `tab.close` — close panes, retaining the last tab as the workspace default.
    pub fn close_tab(&mut self, id: &str) -> Result<(), String> {
        if !self.tabs.contains_key(id) {
            return Err(format!("no tab {id}"));
        }
        if self.is_last_tab(id) {
            let panes = self.tabs[id].panes.clone();
            for pane in panes {
                self.close_terminal(&pane)?;
            }
        } else {
            self.remove_tab(id);
        }
        Ok(())
    }

    /// Close a workspace: kill every tab's panes, drop the tabs, unlink
    /// from the project. The caller decides about the checkout on disk
    /// (git worktree remove for linked workspaces).
    pub fn close_workspace(&mut self, id: &str) -> Result<(), String> {
        let ws = self
            .workspaces
            .remove(id)
            .ok_or_else(|| format!("no workspace {id}"))?;
        for tid in ws.tabs {
            self.remove_tab(&tid);
        }
        for p in self.projects.values_mut() {
            p.workspaces.retain(|x| x != id);
        }
        self.broadcast_event(
            "workspace.removed",
            json!({"workspace_id": id, "project_id": ws.project_id}),
        );
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
        // close_workspace borrows self again — collect ids first
        for wid in p.workspaces {
            if let Some(ws) = self.workspaces.remove(&wid) {
                for tid in ws.tabs.clone() {
                    self.remove_tab(&tid);
                }
                self.broadcast_event(
                    "workspace.removed",
                    json!({"workspace_id": wid, "project_id": id}),
                );
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

    /// The workspace that owns a pane.
    pub fn workspace_of_term(&self, term_id: &str) -> Option<&Workspace> {
        let (_, ws_id, _) = self.term_parents(term_id);
        ws_id.and_then(|id| self.workspaces.get(&id))
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
        let mut emitted = std::collections::HashMap::new();
        // Last detect inputs per terminal — detection is skipped while the
        // screen, the foreground process, and its identity are unchanged.
        let mut seen: std::collections::HashMap<String, (u64, u64, Option<i32>, String, String)> =
            std::collections::HashMap::new();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let terms: Vec<(String, Arc<Mutex<PtyTerm>>)> = lock(&session)
                .terminals
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            emitted.retain(|tid, _| terms.iter().any(|(id, _)| id == tid));
            seen.retain(|tid, _| terms.iter().any(|(id, _)| id == tid));
            // One ps(1) batch for every terminal instead of two spawns each.
            let pgids: Vec<i32> = terms.iter().filter_map(|(_, t)| lock(&t).fg_pgid()).collect();
            let procs = crate::agent::fg_processes(&pgids);
            for (tid, t) in terms {
                // PTY EOF only ends the attach stream. Remove the pane from
                // the session too, using the same cleanup as terminal.close.
                let dead = lock(&t).is_dead();
                if dead {
                    let _ = lock(&session).close_terminal(&tid);
                    emitted.remove(&tid);
                    seen.remove(&tid);
                    continue;
                }
                let (stamp, mut event) = {
                    let mut g = lock(&t);
                    let key = {
                        let pgid = g.fg_pgid();
                        let (comm, args) =
                            pgid.and_then(|p| procs.get(&p)).cloned().unwrap_or_default();
                        (g.input_seq, g.output_seq, pgid, comm, args)
                    };
                    if seen.get(&tid) != Some(&key) {
                        g.detect_agent_with(&key.3, &key.4);
                        seen.insert(tid.clone(), key);
                    }
                    ((g.lifecycle.launch_id.clone(), g.lifecycle.state_seq), json!({
                        "term_id": tid, "agent": g.agent_kind, "agent_status": g.agent_status,
                        "launch_id": g.lifecycle.launch_id, "state_seq": g.lifecycle.state_seq,
                        "source": g.lifecycle.source,
                    }))
                };
                let name = lock(&session).agent_names.iter()
                    .find(|(_, binding)| binding.term_id == tid && stamp.0.as_deref() == Some(binding.launch_id.as_str()))
                    .map(|(name, _)| name.clone());
                event["name"] = json!(name);
                let stamp = (stamp.0, stamp.1, name);
                // API polling may detect a transition before this watcher.
                // Publish based on the last emitted sequence, not just whether
                // this particular detect_agent call changed the state.
                if emitted.get(&tid) != Some(&stamp) {
                    emitted.insert(tid, stamp);
                    lock(&session).broadcast_event("agent.status", event);
                }
            }
        }
    }

    /// Persist the session: model + each pane's raw output tail.
    pub fn persist(&self) -> Result<(), String> {
        let terminals: Vec<PersistedTerm> = self
            .terminals
            .values()
            .map(|t| {
                let mut t = lock(t);
                let (tab_id, _, _) = self.term_parents(&t.id);
                PersistedTerm {
                    id: t.id.clone(),
                    tab_id: tab_id.unwrap_or_default(),
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
            version: SESSION_VERSION,
            next_term: self.next_term,
            next_project: self.next_project,
            next_group: self.next_group,
            next_workspace: self.next_workspace,
            next_tab: self.next_tab,
            focused_project_id: self.focused_project_id.clone(),
            projects: self.projects.values().cloned().collect(),
            groups: self.groups.clone(),
            workspaces: self.workspaces.values().cloned().collect(),
            tabs: self.tabs.values().cloned().collect(),
            terminals,
        };
        let bytes = serde_json::to_vec_pretty(&p).map_err(|e| e.to_string())?;
        state::atomic_write(&state::session_path(), &bytes).map_err(|e| format!("session persist failed: {e}"))
    }

    /// Restore a persisted session — recreates the model and respawns each
    /// pane's shell, replaying its output tail into the screen model.
    /// v1 files (project > terminal) are ignored wholesale.
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
        self.next_term = p.next_term.max(self.next_term);
        self.next_project = p.next_project.max(self.next_project);
        self.next_group = p.next_group.max(self.next_group);
        self.next_workspace = p.next_workspace.max(self.next_workspace);
        self.next_tab = p.next_tab.max(self.next_tab);
        self.focused_project_id = p.focused_project_id;
        for proj in p.projects {
            self.projects.insert(proj.id.clone(), proj);
        }
        self.groups = p.groups;
        for ws in p.workspaces {
            self.workspaces.insert(ws.id.clone(), ws);
        }
        for tab in p.tabs {
            self.tabs.insert(tab.id.clone(), tab);
        }
        // drop memberships pointing at objects that no longer exist
        for g in self.groups.iter_mut() {
            g.projects.retain(|pid| self.projects.contains_key(pid));
        }
        for proj in self.projects.values_mut() {
            proj.workspaces
                .retain(|wid| self.workspaces.contains_key(wid));
        }
        for ws in self.workspaces.values_mut() {
            ws.tabs.retain(|tid| self.tabs.contains_key(tid));
        }
        for pt in p.terminals {
            // skip panes whose tab didn't persist
            let Some(tab) = self.tabs.get(&pt.tab_id) else {
                continue;
            };
            let kind = if self
                .workspaces
                .get(&tab.workspace_id)
                .map(|w| w.is_main)
                .unwrap_or(true)
            {
                "plain"
            } else {
                "worktree"
            };
            let (term, mut reader) = match PtyTerm::spawn(
                &pt.id,
                &pt.cwd,
                &pt.command,
                pt.label.clone(),
                kind,
                pt.cols,
                pt.rows,
                &tab.workspace_id,
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
            if let Some(tab) = self.tabs.get_mut(&pt.tab_id) {
                if !tab.panes.contains(&pt.id) {
                    tab.panes.push(pt.id);
                }
            }
        }
        // drop pane refs to terminals that failed to spawn
        let live: std::collections::HashSet<&String> = self.terminals.keys().collect();
        for tab in self.tabs.values_mut() {
            tab.panes.retain(|id| live.contains(id));
        }
        self.ensure_default_tabs();
    }
}

#[cfg(test)]
mod default_tab_tests {
    use super::*;

    fn project() -> Session {
        let mut session = Session::new();
        session.projects.insert("p1".into(), Project {
            id: "p1".into(), label: None, repo_root: "/tmp".into(),
            repo_key: "/tmp".into(), workspaces: vec![],
        });
        session
    }

    #[test]
    fn last_pane_close_keeps_tab_and_allows_new_shell() {
        let mut s = project();
        let result = s.create_workspace("p1", "/tmp", None, None, true).unwrap();
        let wid = result["workspace"]["workspace_id"].as_str().unwrap();
        let tab = result["tab_id"].as_str().unwrap();
        let tid = result["terminal"]["term_id"].as_str().unwrap();
        s.close_terminal(tid).unwrap();
        assert_eq!(s.workspaces[wid].tabs, vec![tab]);
        assert!(s.tabs[tab].panes.is_empty());
        s.create_terminal(tab, "/tmp", "", None, "plain", 80, 24).unwrap();
        assert_eq!(s.tabs[tab].panes.len(), 1);
        s.close_workspace(wid).unwrap();
        assert!(s.tabs.is_empty());
        assert!(s.terminals.is_empty());
    }

    #[test]
    fn closing_extra_tab_removes_it_but_last_tab_survives() {
        let mut s = project();
        let result = s.create_workspace("p1", "/tmp", None, None, true).unwrap();
        let wid = result["workspace"]["workspace_id"].as_str().unwrap();
        let tab = result["tab_id"].as_str().unwrap();
        let extra = s.create_tab(wid, None, "").unwrap();
        s.close_tab(extra["tab_id"].as_str().unwrap()).unwrap();
        assert_eq!(s.workspaces[wid].tabs, vec![tab]);
        s.close_tab(tab).unwrap();
        s.close_tab(tab).unwrap();
        assert_eq!(s.workspaces[wid].tabs, vec![tab]);
        assert!(s.terminals.is_empty());
        s.close_project("p1").unwrap();
        assert!(s.workspaces.is_empty());
        assert!(s.tabs.is_empty());
    }

    #[test]
    fn empty_extra_tab_is_removed_on_last_pane_exit() {
        let mut s = project();
        let result = s.create_workspace("p1", "/tmp", None, None, true).unwrap();
        let wid = result["workspace"]["workspace_id"].as_str().unwrap();
        let extra = s.create_tab(wid, None, "").unwrap();
        s.close_terminal(extra["terminal"]["term_id"].as_str().unwrap()).unwrap();
        assert!(!s.tabs.contains_key(extra["tab_id"].as_str().unwrap()));
        assert_eq!(s.workspaces[wid].tabs.len(), 1);
        s.close_workspace(wid).unwrap();
    }

    #[test]
    fn missing_default_tab_is_repaired_once() {
        let mut s = project();
        s.workspaces.insert("w1".into(), Workspace {
            id: "w1".into(), project_id: "p1".into(), label: None,
            path: "/tmp".into(), branch: None, is_main: true, tabs: vec![],
        });
        s.ensure_default_tabs();
        let tabs = s.workspaces["w1"].tabs.clone();
        assert_eq!(tabs.len(), 1);
        assert_eq!(s.tabs[&tabs[0]].panes.len(), 1);
        s.ensure_default_tabs();
        assert_eq!(s.workspaces["w1"].tabs, tabs);
        s.close_workspace("w1").unwrap();
    }
}
