#!/usr/bin/env python3
"""Schema check: verify the herdr fields lazed depends on still exist.

Run this before/after upgrading herdr. It exercises the exact JSON paths
lazed reads and fails loudly on drift, so a pre-1.0 schema change shows
up here instead of as a silent runtime break.

Usage: python3 scripts/check_herdr_schema.py [herdr-args...]
       (default targets the running default-session server)
"""

import json
import subprocess
import sys

HERDR_ARGS = sys.argv[1:] or []

FAILURES = []


def cli(*args: str) -> dict:
    out = subprocess.run(
        ["herdr", *HERDR_ARGS, *args], capture_output=True, text=True
    )
    line = next(
        (l for l in out.stdout.splitlines() if l.lstrip().startswith(("{", "["))),
        "",
    )
    if not line:
        FAILURES.append(f"herdr {args}: no JSON output ({out.stderr.strip()})")
        return {}
    try:
        return json.loads(line)
    except json.JSONDecodeError as e:
        FAILURES.append(f"herdr {args}: bad JSON ({e})")
        return {}


def check(name: str, ok: bool, detail: str = ""):
    if ok:
        print(f"  ok   {name}")
    else:
        FAILURES.append(f"{name} {detail}")
        print(f"  FAIL {name} {detail}")


def dig(v, *path):
    for k in path:
        if isinstance(v, dict):
            v = v.get(k)
        elif isinstance(v, list) and isinstance(k, int) and k < len(v):
            v = v[k]
        else:
            return None
    return v


print("herdr status --json")
st = cli("status", "--json")
check("/server/running is bool", isinstance(dig(st, "server", "running"), bool))
check("/server/socket is str", isinstance(dig(st, "server", "socket"), str))
check("/server/version is str", isinstance(dig(st, "server", "version"), str))
check(
    "/server/compatible is bool",
    isinstance(dig(st, "server", "compatible"), bool),
)
check(
    "/server/endpoint_compatible is bool",
    isinstance(dig(st, "server", "endpoint_compatible"), bool),
)

print("herdr api snapshot")
snap = cli("api", "snapshot")
s = dig(snap, "result", "snapshot") or snap.get("result") or snap
check("snapshot.workspaces[]", isinstance(s.get("workspaces"), list))
check("snapshot.tabs[]", isinstance(s.get("tabs"), list))
check("snapshot.panes[]", isinstance(s.get("panes"), list))
check("snapshot.layouts[]", isinstance(s.get("layouts"), list))
check("snapshot.agents[]", isinstance(s.get("agents"), list))
panes = s.get("panes") or []
if panes:
    p = panes[0]
    check("panes[0].pane_id", isinstance(p.get("pane_id"), str))
    check("panes[0].workspace_id", isinstance(p.get("workspace_id"), str))
    check("panes[0].tab_id", isinstance(p.get("tab_id"), str))
layouts = s.get("layouts") or []
if layouts:
    l0 = layouts[0]
    check("layouts[0].panes[]", isinstance(l0.get("panes"), list))
    check("layouts[0].splits[]", isinstance(l0.get("splits"), list))
    if l0.get("panes"):
        lp = l0["panes"][0]
        check("layout pane rect", isinstance(lp.get("rect"), dict))
        if isinstance(lp.get("rect"), dict):
            check(
                "rect x/y/w/h",
                all(
                    isinstance(lp["rect"].get(k), (int, float))
                    for k in ("x", "y", "width", "height")
                ),
            )
else:
    print("  (no layout yet — pane checks skipped)")

print("herdr machine list --json")
ml = cli("machine", "list", "--json")
check("machine list is array", isinstance(ml, list))
if ml:
    m0 = ml[0]
    for k in ("id", "label", "target", "session", "enabled", "selected"):
        check(f"machine row .{k}", k in m0)

print()
if FAILURES:
    print(f"{len(FAILURES)} schema failure(s):")
    for f in FAILURES:
        print(f"  - {f}")
    sys.exit(1)
print("schema check passed")
