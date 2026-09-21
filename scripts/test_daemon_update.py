#!/usr/bin/env python3
"""Update lifecycle checks against a disposable daemon; never uses the live socket."""
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


class DaemonUpdateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="lazed-update-", dir="/private/tmp")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / "lazed"
        shutil.copy2(ROOT / "daemon/target/debug/lazed", self.binary)
        self.env = dict(os.environ, LAZED_STATE_DIR=str(self.root / "state"),
                        LAZED_CONFIG_DIR=str(self.root / "config"),
                        LAZED_WORKTREE_DIR=str(self.root / "worktrees"), SHELL="/bin/sh")
        self.log = (self.root / "daemon.log").open("w")
        self.addCleanup(self.log.close)
        self.start()
        self.addCleanup(self.stop)

    def start(self):
        self.server = subprocess.Popen([str(self.binary), "server", "--foreground"],
                                       env=self.env, stdout=subprocess.DEVNULL,
                                       stderr=self.log)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                self.rpc("session.status")
                return
            except (OSError, ValueError):
                if self.server.poll() is not None:
                    self.fail((self.root / "daemon.log").read_text())
                time.sleep(0.02)
        self.stop()
        self.fail("test daemon did not start")

    def stop(self):
        if self.server.poll() is None:
            for term in self.rpc("session.snapshot")["result"]["terminals"]:
                self.rpc("terminal.close", {"term_id": term["term_id"]})
            self.rpc("server.stop")
            self.server.wait(timeout=5)

    def rpc(self, method, params=None):
        with socket.socket(socket.AF_UNIX) as conn:
            conn.settimeout(5)
            conn.connect(str(self.root / "state/lazed.sock"))
            conn.sendall((json.dumps({"id": 1, "method": method,
                                     "params": params or {}}) + "\n").encode())
            with conn.makefile() as reader:
                line = reader.readline()
            return json.loads(line) if line else None

    def test_empty_stop_persists_and_restart_clears_update(self):
        before = self.rpc("session.status")["result"]
        self.assertIn("server.stop_if_empty.v1", before["capabilities"])
        self.assertFalse(before["binary_updated"])
        stamp = self.binary.stat()
        # A sub-second timestamp change must be detected, too.
        os.utime(self.binary, ns=(stamp.st_atime_ns, stamp.st_mtime_ns + 100_000))
        self.assertTrue(self.rpc("session.status")["result"]["binary_updated"])
        self.assertIsNone(self.rpc("server.stop_if_empty"))
        self.assertEqual(self.server.wait(timeout=5), 0)
        self.assertTrue((self.root / "state/session.json").exists())
        self.start()
        after = self.rpc("session.status")["result"]
        self.assertNotEqual(before["pid"], after["pid"])
        self.assertFalse(after["binary_updated"])

    def test_terminal_created_after_status_blocks_automatic_stop(self):
        self.assertEqual(self.rpc("session.status")["result"]["terms"], 0)
        project = self.rpc("project.create", {"cwd": str(self.root)})
        self.assertIn("result", project, project)
        tid = project["result"]["terminal"]["term_id"]
        refused = self.rpc("server.stop_if_empty")
        self.assertIn("daemon_has_terminals", refused["error"])
        self.assertIsNone(self.server.poll())
        self.assertEqual(self.rpc("session.status")["result"]["terms"], 1)
        self.rpc("terminal.close", {"term_id": tid})
        self.assertIsNone(self.rpc("server.stop_if_empty"))
        self.assertEqual(self.server.wait(timeout=5), 0)


if __name__ == "__main__":
    unittest.main()
