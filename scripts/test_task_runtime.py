#!/usr/bin/env python3
"""Black-box PTY + Git + socket tests in a disposable daemon. No live daemon/API use."""
import concurrent.futures
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "daemon/target/debug/lazed"


class RuntimeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="lazed-task-test-", dir="/private/tmp")
        cls.root = Path(cls.temp.name)
        cls.repo = cls.root / "repo"
        cls.repo.mkdir()
        cls.env = dict(os.environ, LAZED_STATE_DIR=str(cls.root / "state"),
                       LAZED_CONFIG_DIR=str(cls.root / "config"),
                       LAZED_WORKTREE_DIR=str(cls.root / "worktrees"),
                       PI_CODING_AGENT_DIR=str(cls.root / "pi-state"), SHELL="/bin/sh")
        cls.git("init", "-b", "main")
        cls.git("-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                "commit", "--allow-empty", "-m", "fixture")
        fake = cls.root / "bin/codex"
        fake.parent.mkdir()
        shutil.copyfile(ROOT / "scripts/fixtures/codex", fake)
        fake.chmod(0o700)
        history_fake = cls.root / "bin/claude"
        shutil.copyfile(ROOT / "scripts/fixtures/claude", history_fake)
        history_fake.chmod(0o700)
        config = Path(cls.env["LAZED_CONFIG_DIR"])
        config.mkdir()
        specs = {
            name: {"kind": str(fake), "args": flags} for name, flags in {
                "test": [], "blocked": ["--blocked"], "fast": ["--fast"], "silent": ["--silent"],
            }.items()
        }
        specs["history"] = {"kind": str(history_fake), "args": []}
        specs["scrolled"] = {"kind": str(history_fake), "args": ["--scrolled"]}
        (config / "agents.json").write_text(json.dumps({"default": "test", "specs": specs}))
        cls.log = open(cls.root / "server.log", "ab")
        try:
            cls.start_server()
        except Exception:
            if cls.server.poll() is None:
                cls.server.terminate()
                cls.server.wait(timeout=5)
            cls.log.close()
            print((cls.root / "server.log").read_text())
            cls.temp.cleanup()
            raise

    @classmethod
    def git(cls, *args):
        return subprocess.check_output(["git", "-C", str(cls.repo), *args], stderr=subprocess.STDOUT).decode().strip()

    @classmethod
    def start_server(cls):
        cls.server = subprocess.Popen([str(BINARY), "server", "--foreground"], env=cls.env,
                                      stdout=cls.log, stderr=cls.log)
        for _ in range(100):
            try:
                cls.rpc("session.status")
                return
            except (OSError, ValueError):
                time.sleep(0.05)
        raise RuntimeError("isolated daemon failed to start")

    @classmethod
    def rpc(cls, method, params=None, error=False):
        with socket.socket(socket.AF_UNIX) as conn:
            conn.settimeout(45)
            conn.connect(str(Path(cls.env["LAZED_STATE_DIR"]) / "lazed.sock"))
            conn.sendall((json.dumps({"id": 1, "method": method, "params": params or {}}) + "\n").encode())
            result = json.loads(conn.makefile().readline())
        if error:
            if "error" not in result:
                raise AssertionError(f"expected error, got {result}")
            return result["error"]
        if "error" in result:
            raise AssertionError(result["error"])
        return result["result"]

    @classmethod
    def tearDownClass(cls):
        try:
            # Close only panes owned by this isolated test daemon.
            for term in cls.rpc("session.snapshot")["terminals"]:
                cls.rpc("terminal.close", {"term_id": term["term_id"]})
            cls.rpc("server.stop")
            cls.server.wait(timeout=5)
        finally:
            if cls.server.poll() is None:
                cls.server.terminate()
                cls.server.wait(timeout=5)
            cls.log.close()
            cls.temp.cleanup()

    def test_reliability_invalid_sizes_and_failed_persist(self):
        project = self.rpc("project.create", {"cwd": str(self.repo)})["project"]["project_id"]
        before = self.rpc("session.snapshot")
        for cols, rows in [(0, 24), (1, 24), (80, 0), (65536, 24), (1000, 500)]:
            self.rpc("terminal.create", {"project_id": project, "cols": cols, "rows": rows}, error=True)
        self.assertEqual(len(before["terminals"]), len(self.rpc("session.snapshot")["terminals"]))
        pane = self.rpc("terminal.create", {"project_id": project, "cols": 90, "rows": 30})["term_id"]
        self.rpc("terminal.resize", {"term_id": pane, "cols": 0, "rows": 24}, error=True)
        self.rpc("terminal.resize", {"term_id": pane, "cols": 80, "rows": 24})
        self.rpc("terminal.close", {"term_id": pane})
        self.rpc("session.persist")
        path = Path(self.env["LAZED_STATE_DIR"]) / "session.json"
        saved = path.with_suffix(".saved")
        path.rename(saved)
        path.mkdir()
        try:
            self.rpc("session.persist", error=True)
            self.rpc("server.stop", error=True)
            self.rpc("session.status")
            self.assertTrue(saved.is_file())
            self.assertFalse(list(path.parent.glob(".state-*.tmp")))
        finally:
            path.rmdir()
            saved.rename(path)

    def test_reliability_git_hook_does_not_block_session(self):
        hook = self.repo / ".git/hooks/post-checkout"
        started, release = self.root / "hook-started", self.root / "hook-release"
        hook.write_text(f'#!/bin/sh\ntouch "{started}"\nwhile [ ! -e "{release}" ]; do sleep 0.05; done\n')
        hook.chmod(0o700)
        with concurrent.futures.ThreadPoolExecutor() as pool:
            creating = pool.submit(self.rpc, "workspace.create", {"repo": str(self.repo), "branch": "slow-hook"})
            try:
                deadline = time.monotonic() + 5
                while not started.exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
                self.assertTrue(started.exists())
                status = pool.submit(self.rpc, "session.status")
                status.result(timeout=2)
            finally:
                release.touch()
                hook.unlink()
            creating.result(timeout=10)

    def test_reliability_refused_uninstall_keeps_daemon(self):
        home = self.root / "uninstall-home"
        link = home / ".local/bin/lazed"
        link.parent.mkdir(parents=True)
        link.symlink_to(BINARY)
        env = dict(self.env, HOME=str(home))
        pid = self.rpc("session.status")["pid"]
        run = subprocess.run([str(BINARY), "uninstall"], env=env, input="", text=True, capture_output=True, timeout=5)
        self.assertNotEqual(run.returncode, 0)
        self.assertTrue(link.is_symlink())
        self.assertEqual(self.rpc("session.status")["pid"], pid)

    def test_shell_exit_removes_pane_and_empty_tab(self):
        project = self.rpc("project.create", {"cwd": str(self.repo)})
        first = self.rpc("terminal.create", {"project_id": project["project"]["project_id"], "command": "/bin/sh"})
        second = self.rpc("terminal.create", {"tab_id": first["tab_id"], "command": "/bin/sh"})
        with socket.socket(socket.AF_UNIX) as events:
            events.settimeout(5)
            events.connect(str(Path(self.env["LAZED_STATE_DIR"]) / "lazed.sock"))
            events.sendall(b'{"id":1,"method":"events.subscribe"}\n')
            stream = events.makefile()
            json.loads(stream.readline())
            for term in (first, second):
                self.rpc("terminal.input", {"term_id": term["term_id"], "text": "exit\r"})
                while True:
                    event = json.loads(stream.readline())
                    if event.get("event") == "terminal.closed" and event["data"]["term_id"] == term["term_id"]:
                        break
                snapshot = self.rpc("session.snapshot")
                self.assertNotIn(term["term_id"], [t["term_id"] for t in snapshot["terminals"]])
                tabs = {t["tab_id"]: t for t in snapshot["tabs"]}
                if term is first:
                    self.assertEqual(tabs[first["tab_id"]]["panes"], [second["term_id"]])
                else:
                    self.assertNotIn(first["tab_id"], tabs)
            stream.close()

    def start(self, id, spec="test", text="한글 요청 'quotes'\n두 번째 줄"):
        p = {"cwd": str(self.repo), "request_id": id, "spec": spec, "text": text}
        return self.rpc("task.start", p), p

    def settled(self, id):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            task = self.rpc("task.status", {"task_id": id})
            if task["phase"] == "settled":
                return task
            time.sleep(0.1)
        self.fail(f"not settled: {task}")

    def test_01_start_paste_dedupe_followup(self):
        text = "한글 'quotes' $HOME `literal`\n" + "긴 요청 " * 900
        task, params = self.start("main-task", text=text)
        self.assertEqual(task["phase"], "running", task)
        self.assertEqual(task["base_commit"], self.git("rev-parse", "HEAD"))
        self.assertIn("task_not_settled", self.rpc("task.tell", {"task_id": "main-task", "request_id": "busy", "text": "NO"}, error=True))
        with concurrent.futures.ThreadPoolExecutor() as pool:
            duplicates = list(pool.map(lambda _: self.rpc("task.start", params), range(3)))
        self.assertTrue(all(d["term_id"] == task["term_id"] for d in duplicates))
        self.assertIn("request_id_conflict", self.rpc("task.start", dict(params, text="different"), error=True))
        task = self.settled("main-task")
        received = Path(task["checkout_path"]) / "received.jsonl"
        self.assertEqual([json.loads(line) for line in received.read_text().splitlines()], [text])
        self.assertFalse(task["verified"])
        tell = {"task_id": "main-task", "request_id": "followup", "text": "추가 요청"}
        self.rpc("task.tell", tell)
        self.rpc("task.tell", tell)
        self.settled("main-task")
        self.assertEqual(len(received.read_text().splitlines()), 2)
        self.assertIn("stale_launch", self.rpc("agent.report", {"term_id": task["term_id"], "launch_id": "old", "seq": 5, "state": "idle"}, error=True))

    def test_02_blocked_then_explicit_resume(self):
        task, params = self.start("blocked-task", "blocked")
        self.assertEqual(task["phase"], "awaiting_ready", task)
        self.assertFalse((Path(task["checkout_path"]) / "received.jsonl").exists())
        self.assertEqual(self.rpc("task.start", params)["term_id"], task["term_id"])
        self.rpc("terminal.input", {"term_id": task["term_id"], "text": "1\r"})
        resumed = self.rpc("task.resume", {"task_id": task["task_id"]})
        self.assertEqual(resumed["phase"], "running", resumed)
        self.settled(task["task_id"])

    def test_03_fast_hook_and_old_idle_wait(self):
        task, _ = self.start("fast-task", "fast")
        self.assertEqual(task["phase"], "settled", task)
        self.assertGreater(task["agent"]["activity_seq"], task["receipt"]["after_seq"])
        self.assertEqual(task["agent"]["source"], "hook")
        self.assertIn("agent_wait_timeout", self.rpc("agent.wait", {"term_id": task["term_id"], "launch_id": task["launch_id"], "after_seq": task["agent"]["state_seq"], "timeout_ms": 150}, error=True))

    def test_04_dirty_source_and_control_characters(self):
        dirty = self.repo / "dirty.txt"
        dirty.write_text("do not copy")
        p = {"cwd": str(self.repo), "request_id": "dirty-task", "spec": "fast", "text": "test"}
        try:
            self.assertIn("dirty_source", self.rpc("task.start", p, error=True))
            task = self.rpc("task.start", dict(p, allow_dirty=True))
            self.assertTrue(task["warnings"])
            self.assertFalse((Path(task["checkout_path"]) / "dirty.txt").exists())
        finally:
            dirty.unlink()
        self.assertIn("control characters", self.rpc("task.start", dict(p, request_id="control-task", text="\x1b[201~bad"), error=True))
        self.assertIn("unknown agent spec", self.rpc("task.start", dict(p, request_id="bad-spec", spec="missing"), error=True))
        self.assertFalse((self.root / "worktrees/bad-spec").exists())

    def test_05_uncertain_no_automatic_resend(self):
        task, p = self.start("silent-task", "silent")
        self.assertEqual(task["phase"], "submission_uncertain", task)
        duplicate = self.rpc("task.start", p)
        self.assertEqual(duplicate["phase"], "submission_uncertain")
        self.assertIn("submission_pending", self.rpc("agent.prompt", {"term_id": task["term_id"], "text": "NO RESEND"}, error=True))
        self.assertIn("resume only", self.rpc("task.resume", {"task_id": task["task_id"]}, error=True))
        self.assertEqual(len((Path(task["checkout_path"]) / "received.jsonl").read_text().splitlines()), 1)

    def test_06_dirty_remove_preserves_pane(self):
        task = self.rpc("task.status", {"task_id": "main-task"})
        self.assertIn("workspace_has_agent", self.rpc("workspace.remove", {"workspace_id": task["workspace_id"]}, error=True))
        self.rpc("terminal.input", {"term_id": task["term_id"], "text": "\x03"})
        time.sleep(0.3)
        self.assertIn("git worktree remove failed", self.rpc("workspace.remove", {"workspace_id": task["workspace_id"]}, error=True))
        ids = [t["term_id"] for t in self.rpc("session.snapshot")["terminals"]]
        self.assertIn(task["term_id"], ids)
        self.assertEqual(self.rpc("task.status", {"task_id": "main-task"})["phase"], "interrupted")

    def test_07_cli_and_restart(self):
        run = subprocess.run([str(BINARY), "task", "start", "--cwd", str(self.repo), "--agent", "fast", "--request-id", "cli-task", "--", "CLI 요청"], env=self.env, text=True, capture_output=True)
        self.assertEqual(run.returncode, 0, run.stderr + run.stdout)
        self.assertEqual(json.loads(run.stdout)["phase"], "settled")
        for term in self.rpc("session.snapshot")["terminals"]:
            self.rpc("terminal.close", {"term_id": term["term_id"]})
        self.rpc("server.stop")
        self.server.wait(timeout=5)
        type(self).start_server()
        task = self.rpc("task.status", {"task_id": "cli-task"})
        self.assertEqual(task["phase"], "interrupted")
        self.assertIn("interrupted", self.rpc("task.resume", {"task_id": "cli-task"}, error=True))

    def cli(self, *args, ok=True, cwd=None, env=None):
        run = subprocess.run([str(BINARY), *args], env=env or self.env, cwd=cwd or self.repo,
                             text=True, capture_output=True, timeout=40)
        if not ok:
            self.assertNotEqual(run.returncode, 0, run.stdout)
            return run.stderr
        self.assertEqual(run.returncode, 0, run.stderr + run.stdout)
        return json.loads(run.stdout)

    def test_06b_named_sibling_workflow_preserves_location_and_focus(self):
        snap = self.rpc("session.snapshot")
        main_ids = {w["workspace_id"] for w in snap["workspaces"] if w["is_main"] and w["path"] == str(self.repo)}
        # The real-Pi smoke test may leave an empty main workspace. Choose
        # an actual caller pane, independent of HashMap iteration order.
        tab = next(t for t in snap["tabs"] if t["workspace_id"] in main_ids and t["panes"])
        parent = tab["panes"][0]
        subdir = self.repo / "caller-subdir"
        subdir.mkdir()
        pane = None
        try:
            result = self.cli("pane", "split", "--current", "--no-focus", cwd=subdir,
                              env=dict(self.env, LAZED_TERM=parent))
            pane = result["pane"]
            self.assertEqual(pane["tab_id"], tab["tab_id"])
            self.assertEqual(pane["cwd"], str(subdir))
            after = self.rpc("session.snapshot")
            self.assertEqual(len(after["workspaces"]), len(snap["workspaces"]))
            self.assertEqual(after.get("focused_project_id"), snap.get("focused_project_id"))
            tid = pane["term_id"]
            started = self.cli("agent", "start", "reviewer", "--spec", "test", "--pane", tid)
            self.assertEqual(started["name"], "reviewer")
            ui_term = next(t for t in self.rpc("session.snapshot")["terminals"] if t["term_id"] == tid)
            self.assertEqual(ui_term["agent_name"], "reviewer")
            self.assertIn("agent_name_in_use", self.cli("agent", "start", "reviewer", "--spec", "test", "--pane", parent, ok=False))
            receipt = self.cli("agent", "prompt", "reviewer", "현재 변경 리뷰\n추가 줄", "--wait", "--timeout", "10000")
            self.assertEqual(receipt["observed"]["agent_status"], "done", receipt)
            self.assertEqual(receipt["name"], "reviewer")
            self.assertIn("Response ended", self.cli("agent", "read", "reviewer")["text"])
            self.cli("agent", "prompt", "reviewer", "추가 확인", "--wait", "--timeout", "10000")
            self.assertEqual(len((subdir / "received.jsonl").read_text().splitlines()), 2)
            self.assertTrue(any(a.get("name") == "reviewer" for a in self.cli("agent", "list")))
            self.assertIn("unknown logical key", self.cli("agent", "send-keys", "reviewer", "enter", "not-a-key", ok=False))
            self.cli("agent", "send-keys", "reviewer", "ctrl+c")
            time.sleep(0.3)
            self.assertIn("agent_name_expired", self.cli("agent", "get", "reviewer", ok=False))
            self.cli("agent", "start", "reviewer", "--spec", "fast", "--pane", tid)
            self.cli("agent", "prompt", "reviewer", "new occupant", "--wait", "--timeout", "10000")
        finally:
            if pane:
                self.rpc("terminal.close", {"term_id": pane["term_id"]})
            (subdir / "received.jsonl").unlink(missing_ok=True)
            subdir.rmdir()

    def test_06c_explicit_worktree_is_separate_from_agent_start(self):
        result = self.cli("worktree", "create", "--branch", "named-worktree")
        tid = result["terminal"]["term_id"]
        self.assertIsNone(self.rpc("agent.get", {"term_id": tid})["agent"])
        self.assertEqual(result["base_commit"], self.git("rev-parse", "HEAD"))
        self.assertIn("already exists", self.cli("worktree", "create", "--branch", "named-worktree", ok=False))
        started = self.cli("agent", "start", "backend", "--spec", "blocked", "--pane", tid, ok=False)
        self.assertIn("agent_blocked", started)
        self.assertEqual(self.cli("agent", "get", "backend")["agent_status"], "blocked")
        self.assertIn("trust this folder", self.cli("agent", "read", "backend")["text"])
        self.cli("agent", "send-keys", "backend", "1", "enter")
        self.cli("agent", "wait", "backend", "--until", "idle", "--timeout", "10000")
        receipt = self.cli("agent", "prompt", "backend", "worktree request", "--wait", "--timeout", "10000")
        self.assertEqual(receipt["observed"]["agent_status"], "done")
        self.assertTrue((Path(result["checkout_path"]) / "received.jsonl").exists())

    def test_06d_named_wait_does_not_accept_old_idle(self):
        pane = self.rpc("terminal.create", {"project_id": self.rpc("session.snapshot")["projects"][0]["project_id"]})
        tid = pane["term_id"]
        try:
            self.cli("agent", "start", "silent", "--spec", "silent", "--pane", tid)
            self.cli("agent", "prompt", "silent", "stalled", "--wait", "--timeout", "300", ok=False)
            self.assertIn("agent_wait_timeout", self.cli("agent", "wait", "silent", "--timeout", "150", ok=False))
        finally:
            self.rpc("terminal.close", {"term_id": tid})
            (self.repo / "received.jsonl").unlink(missing_ok=True)

    @unittest.skipUnless(os.environ.get("LAZED_TEST_PI"), "optional real Pi readiness smoke test")
    def test_06a_real_pi_readiness_without_model_request(self):
        project = self.rpc("project.create", {"cwd": str(self.repo)})
        tid = project["terminal"]["term_id"]
        ready = self.rpc("agent.start", {"term_id": tid, "kind": os.environ["LAZED_TEST_PI"], "expected_kind": "pi",
            "args": ["--no-extensions", "--no-skills", "--no-prompt-templates", "--no-themes", "--no-session"]})
        self.assertEqual(ready["agent"], "pi", ready)
        self.assertEqual(ready["source"], "hook", ready)
        self.rpc("terminal.close", {"term_id": tid})

    def test_08_busy_followup_while_first_caller_is_waiting(self):
        task_dir = self.root / "busy-followup"
        task_dir.mkdir()
        pane = self.rpc("project.create", {"cwd": str(task_dir)})["terminal"]["term_id"]
        try:
            self.cli("agent", "start", "busy-reviewer", "--spec", "test", "--pane", pane)
            with concurrent.futures.ThreadPoolExecutor() as pool:
                first = pool.submit(self.cli, "agent", "prompt", "busy-reviewer", "first", "--wait", "--timeout", "10000")
                deadline = time.monotonic() + 5
                while self.cli("agent", "get", "busy-reviewer")["agent_status"] != "working":
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(0.03)
                start = time.monotonic()
                second = self.cli("agent", "prompt", "busy-reviewer", "회귀 테스트도 확인해줘")
                self.assertLess(time.monotonic() - start, 1.0)
                self.assertTrue(second["submitted"])
                self.assertTrue(second["started_while_working"])
                self.assertEqual(second["wait_scope"], "agent_lifecycle")
                first.result(timeout=15)
            received = task_dir / "received.jsonl"
            deadline = time.monotonic() + 5
            while len(received.read_text().splitlines()) < 2:
                self.assertLess(time.monotonic(), deadline)
                time.sleep(0.05)
            self.assertEqual([json.loads(x) for x in received.read_text().splitlines()], ["first", "회귀 테스트도 확인해줘"])
            self.assertEqual(self.cli("agent", "get", "busy-reviewer")["term_id"], pane)
        finally:
            self.rpc("terminal.close", {"term_id": pane})

    def history_pane(self, name, spec="history"):
        project = self.rpc("project.create", {"cwd": str(self.repo)})
        pane = project["terminal"]["term_id"]
        self.cli("agent", "start", name, "--spec", spec, "--pane", pane)
        return pane

    def test_09_history_harvest_and_viewport_restore(self):
        pane = self.history_pane("transcript")
        try:
            before = self.cli("agent", "read", "transcript", "--source", "visible")["text"]
            result = self.cli("agent", "read", "transcript", "--lines", "100")
            self.assertEqual(result["history"]["method"], "alternate-screen", result)
            self.assertTrue(result["history"]["viewport_restored"])
            for i in range(90):
                self.assertEqual(result["text"].count(f"history-row-{i:03d}"), 1)
            after = self.cli("agent", "read", "transcript", "--source", "visible")["text"]
            self.assertEqual(before, after)
            self.assertEqual(self.cli("agent", "get", "transcript")["agent_status"], "idle")
            launch = self.cli("agent", "get", "transcript")["launch_id"]
            self.rpc("agent.report", {"term_id": pane, "launch_id": launch, "seq": 1, "state": "working"})
            self.assertIn("agent_not_idle", self.cli("agent", "read", "transcript", "--lines", "100", ok=False))
            self.assertIn("history-row", self.cli("agent", "read", "transcript", "--source", "visible")["text"])
        finally:
            self.rpc("terminal.close", {"term_id": pane})

    def test_10_history_preserves_manual_viewport(self):
        pane = self.history_pane("manual-scroll", "scrolled")
        try:
            before = self.cli("agent", "read", "manual-scroll", "--source", "visible")["text"]
            result = self.cli("agent", "read", "manual-scroll", "--lines", "100")
            self.assertEqual(result["history"]["method"], "passive")
            self.assertTrue(result["history"]["viewport_restored"])
            self.assertEqual(before, self.cli("agent", "read", "manual-scroll", "--source", "visible")["text"])
        finally:
            self.rpc("terminal.close", {"term_id": pane})

    def test_11_history_yields_to_user_input(self):
        pane = self.history_pane("cancel-read")
        try:
            with concurrent.futures.ThreadPoolExecutor() as pool:
                reading = pool.submit(self.cli, "agent", "read", "cancel-read", "--lines", "100")
                time.sleep(0.7)
                self.rpc("terminal.input", {"term_id": pane, "text": "x"})
                result = reading.result(timeout=10)
            self.assertEqual(result["history"]["method"], "passive")
            self.assertFalse(result["history"]["viewport_restored"])
            self.assertIn("cancelled", result["history"]["note"])
        finally:
            self.rpc("terminal.close", {"term_id": pane})

    def test_12_restart_retains_layout_files_and_output_not_live_names(self):
        pane = self.rpc("project.create", {"cwd": str(self.repo)})["terminal"]["term_id"]
        sibling = self.rpc("pane.split", {"term_id": pane, "cwd": str(self.repo)})["pane"]["term_id"]
        self.rpc("terminal.input", {"term_id": pane, "text": "printf 'RESTORE_MARKER\\n'\r"})
        self.cli("agent", "start", "before-restart", "--spec", "test", "--pane", sibling)
        marker = self.repo / "restart-proof.txt"
        marker.write_text("uncommitted content survives")

        def shape(snapshot):
            return {kind: sorted(item[key] for item in snapshot[kind]) for kind, key in (
                ("projects", "project_id"), ("workspaces", "workspace_id"),
                ("tabs", "tab_id"), ("terminals", "term_id"))}

        before = shape(self.rpc("session.snapshot"))
        self.rpc("server.stop")
        self.server.wait(timeout=5)
        type(self).start_server()
        self.assertEqual(before, shape(self.rpc("session.snapshot")))
        self.assertEqual(marker.read_text(), "uncommitted content survives")
        self.assertIn("RESTORE_MARKER", self.rpc("agent.read", {"term_id": pane, "source": "recent"})["text"])
        self.assertFalse(any(a.get("name") == "before-restart" for a in self.rpc("agent.list")))
        self.assertIn("agent.busy_prompt.v1", self.rpc("session.status")["capabilities"])
        self.assertIn("agent.history.v1", self.rpc("session.status")["capabilities"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
