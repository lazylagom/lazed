#!/usr/bin/env python3
"""Measure the terminal frame-stream path under load (PLAN §7).

Spins up a throwaway `herdr --session lazed-perf` server, floods a pane
with `seq 1 N`, and measures what lazed's pipeline would see:

  - frames/sec and decoded MB/sec through `terminal session control`
  - wall time to drain the flood (seq + a queued echo marker)
  - seq discontinuities (dropped frames)
  - idle round-trip: terminal.input → marker observed in a frame

Usage: python3 scripts/perf_stream.py [--lines 300000] [--session lazed-perf]
"""

import argparse
import base64
import json
import subprocess
import sys
import threading
import time


def cli(session: str, *args: str, timeout: int = 30) -> dict:
    out = subprocess.run(
        ["herdr", "--session", session, *args],
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    line = next(
        (l for l in out.stdout.splitlines() if l.lstrip().startswith("{")), ""
    )
    if not line:
        # some commands (pane run) succeed silently — no JSON at all
        if out.returncode == 0:
            return {}
        raise RuntimeError(f"herdr {args} produced no JSON: {out.stderr.strip()}")
    v = json.loads(line)
    if v.get("error"):
        raise RuntimeError(f"herdr {args}: {v['error']}")
    return v


class ControlStream:
    """`terminal session control` child; reader thread counts frames/bytes
    and watches a decoded text window for markers."""

    def __init__(self, session: str, pane: str, cols: int, rows: int):
        self.proc = subprocess.Popen(
            [
                "herdr",
                "--session",
                session,
                "terminal",
                "session",
                "control",
                pane,
                "--takeover",
                "--cols",
                str(cols),
                "--rows",
                str(rows),
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        self.frames = 0
        self.bytes_decoded = 0
        self.seq_gaps = 0
        self._last_seq = None
        self._buf = b""
        self.closed = threading.Event()
        self._t = threading.Thread(target=self._read, daemon=True)
        self._t.start()

    def _read(self):
        assert self.proc.stdout
        for raw in self.proc.stdout:
            raw = raw.strip()
            if not raw:
                continue
            try:
                msg = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if msg.get("type") == "terminal.frame" and msg.get("bytes"):
                self.frames += 1
                seq = msg.get("seq")
                if (
                    isinstance(seq, int)
                    and self._last_seq is not None
                    and seq != self._last_seq + 1
                ):
                    self.seq_gaps += 1
                if isinstance(seq, int):
                    self._last_seq = seq
                data = base64.b64decode(msg["bytes"])
                self.bytes_decoded += len(data)
                self._buf = (self._buf + data)[-65536:]
            elif msg.get("type") == "terminal.closed":
                self.closed.set()
                return
        self.closed.set()

    def send(self, obj: dict):
        assert self.proc.stdin
        self.proc.stdin.write(json.dumps(obj).encode() + b"\n")
        self.proc.stdin.flush()

    def input_text(self, text: str):
        self.send({"type": "terminal.input", "text": text})

    def wait_for(self, marker: str, timeout: float) -> float | None:
        """Seconds until `marker` appears in decoded output, else None."""
        needle = marker.encode()
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if needle in self._buf:
                return timeout - (deadline - time.monotonic())
            if self.closed.is_set():
                return None
            time.sleep(0.005)
        return None

    def close(self):
        try:
            self.send({"type": "terminal.release"})
        except Exception:
            pass
        self.proc.terminate()
        try:
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--lines", type=int, default=300_000)
    ap.add_argument("--session", default="lazed-perf")
    ap.add_argument("--cols", type=int, default=200)
    ap.add_argument("--rows", type=int, default=50)
    ap.add_argument("--timeout", type=int, default=120)
    ap.add_argument(
        "--steady",
        type=int,
        default=10,
        help="seconds of continuous full-screen redraws to measure (0 = skip)",
    )
    args = ap.parse_args()
    s = args.session

    running = (
        cli(s, "status", "--json")
        .get("server", {})
        .get("running", False)
    )
    spawned = None
    if not running:
        spawned = subprocess.Popen(
            ["herdr", "--session", s, "server"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        for _ in range(40):
            time.sleep(0.25)
            if cli(s, "status", "--json").get("server", {}).get("running"):
                break
        else:
            print("server did not come up", file=sys.stderr)
            return 1

    stream = None
    try:
        cli(s, "workspace", "create", "--label", "perf")
        panes = cli(s, "api", "snapshot").get("result", {}).get("snapshot", {})
        pane_ids = [p["pane_id"] for p in panes.get("panes", [])]
        pane = pane_ids[-1]
        print(f"pane: {pane} ({args.cols}x{args.rows})")

        stream = ControlStream(s, pane, args.cols, args.rows)
        time.sleep(0.5)  # let the initial full frame land

        # ── flood ──────────────────────────────────────────────
        n = args.lines
        t0 = time.monotonic()
        cli(s, "pane", "run", pane, f"seq 1 {n}")
        # Queue the end-marker through the input path — the shell runs it
        # only after seq drains, so marker-arrival ≈ flood wall time.
        # The marker is split ("__DO""NE") so the *echoed input line* does
        # not itself contain the needle — only its output does. `clear`
        # first so the marker lands on a clean line: frames are ANSI diffs,
        # and a marker printed over repainted rows can arrive split.
        stream.input_text(f'clear; echo "__DO""NE_{n}__"\r')
        dt = stream.wait_for(f"__DONE_{n}__", timeout=args.timeout)
        wall = time.monotonic() - t0
        if dt is None:
            print(f"flood did not finish within {args.timeout}s", file=sys.stderr)
            print(f"  partial: {stream.frames} frames, "
                  f"{stream.bytes_decoded / 1e6:.1f} MB in {wall:.1f}s")
            return 1

        # ── idle round-trip ────────────────────────────────────
        lat = None
        for nl in ("\r", "\n"):
            ts = int(time.time() * 1000)
            m = f"__RT_{ts}__"
            t3 = time.monotonic()
            stream.input_text(f'clear; echo "__R""T_{ts}__"{nl}')
            d = stream.wait_for(m, timeout=10)
            if d is not None:
                lat = time.monotonic() - t3
                break
        time.sleep(0.3)  # drain any stragglers before reporting

        # ── steady redraws ─────────────────────────────────────
        # seq floods coalesce into a handful of viewport diffs — that path
        # never stresses the client. Sustained TUI-style repaints do: this
        # measures the frames/sec the stream actually sustains.
        steady = None
        if args.steady > 0:
            m = f"__STEADY_{n}__"
            f0, b0 = stream.frames, stream.bytes_decoded
            t5 = time.monotonic()
            cli(
                s,
                "pane",
                "run",
                pane,
                f"for i in $(seq 1 {args.steady * 200}); do "
                f"printf '\\033[H'; for r in $(seq 1 {args.rows - 5}); do "
                f"printf 'row %s %d\\n' $r $i; done; done; "
                f'clear; echo "__STEADY_""{n}__"',
            )
            if stream.wait_for(m, timeout=args.steady + 60) is None:
                print("steady phase marker never arrived", file=sys.stderr)
            else:
                elapsed = time.monotonic() - t5
                df, db = stream.frames - f0, stream.bytes_decoded - b0
                steady = (elapsed, df, db)

        print(f"\nflood: seq 1 {n}")
        print(f"  frames:        {stream.frames:,}")
        print(f"  decoded:       {stream.bytes_decoded / 1e6:.2f} MB")
        print(f"  wall time:     {wall:.2f}s")
        print(f"  throughput:    {stream.frames / wall:,.0f} frames/s, "
              f"{stream.bytes_decoded / wall / 1e6:.1f} MB/s")
        print(f"  seq gaps:      {stream.seq_gaps}")
        if lat is not None:
            print(f"  round-trip:    {lat * 1000:.0f} ms (input → frame echo)")
        else:
            print("  round-trip:    marker never arrived")
        if steady:
            elapsed, df, db = steady
            print(f"\nsteady redraw ({elapsed:.1f}s of full-screen repaints):")
            print(f"  frames:        {df:,}  ({df / elapsed:,.0f} frames/s)")
            print(f"  decoded:       {db / 1e6:.2f} MB  ({db / elapsed / 1e3:,.0f} KB/s)")
            print(f"  avg frame:     {db / max(df, 1):,.0f} B")
        return 0
    finally:
        if stream:
            stream.close()
        if spawned:
            cli(s, "server", "stop")
            spawned.terminate()
            try:
                spawned.wait(timeout=5)
            except subprocess.TimeoutExpired:
                spawned.kill()


if __name__ == "__main__":
    sys.exit(main())
