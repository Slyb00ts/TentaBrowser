#!/usr/bin/env python3
"""Generate idl/MANIFEST.toml from the golden-site API inventories.

Reads every ``golden/inventory/*.json`` produced by ``golden/tracer`` and the
hand-maintained ``idl/phase-rules.toml``, and writes one manifest entry per
interface seen in any inventory: the WebIDL file it will live in, the phase
that ships it, the owning crate, the WPT directory, the sites it was seen in
and how many members were touched. Interfaces that no rule assigns, or whose
family is marked ``unscheduled``, go to the ``[[unassigned]]`` section for a
human to schedule.

Usage:
    python3 idl/gen_manifest.py            # rewrite idl/MANIFEST.toml
    python3 idl/gen_manifest.py --check    # exit 1 if the committed manifest is stale

Standard library only (Python 3.11+ for tomllib).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
INVENTORY_DIR = ROOT / "golden" / "inventory"
RULES_FILE = ROOT / "idl" / "phase-rules.toml"
MANIFEST_FILE = ROOT / "idl" / "MANIFEST.toml"

PHASE_ORDER = ["P0", "P1", "P2", "P3a", "P3b", "P3c", "P4", "P5", "P6a", "P6b", "P7", "P8", "unscheduled"]
TOP_MEMBERS = 8


# ---- rules -----------------------------------------------------------------


class Rule:
    def __init__(self, raw: dict, index: int) -> None:
        self.index = index
        self.pattern = re.compile(raw["match"])
        self.kind = raw.get("kind", "webidl")
        self.spec = raw.get("spec", "")
        self.phase = raw["phase"]
        self.crate = raw.get("crate", "")
        self.wpt = raw.get("wpt", "")
        self.note = raw.get("note", "")
        if self.phase not in PHASE_ORDER:
            raise SystemExit(f"phase-rules.toml rule {index} ({raw['match']}): unknown phase {self.phase!r}")
        if self.kind == "webidl" and self.phase != "unscheduled" and not self.spec:
            raise SystemExit(f"phase-rules.toml rule {index} ({raw['match']}): spec is required")

    def matches(self, interface: str) -> bool:
        return self.pattern.fullmatch(interface) is not None


class MemberRule:
    def __init__(self, raw: dict) -> None:
        self.interface = raw["interface"]
        self.pattern = re.compile(raw["match"])
        self.phase = raw["phase"]
        self.crate = raw.get("crate", "")
        self.note = raw.get("note", "")


def load_rules(path: Path) -> tuple[list[Rule], list[MemberRule], dict]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    rules = [Rule(r, i) for i, r in enumerate(data.get("rule", []))]
    member_rules = [MemberRule(r) for r in data.get("member_rule", [])]
    return rules, member_rules, data


# ---- inventories -----------------------------------------------------------


class Seen:
    """Aggregated view of one interface across inventories."""

    def __init__(self, name: str) -> None:
        self.name = name
        self.sites: set[str] = set()
        self.members: dict[str, int] = {}
        self.realms: set[str] = set()
        self.accesses = 0

    def add(self, site: str, entry: dict) -> None:
        self.sites.add(site)
        self.accesses += entry.get("total", 0)
        self.realms.update(entry.get("realms", {}).keys())
        for member, counts in entry.get("members", {}).items():
            n = sum(v for k, v in counts.items() if k in ("get", "set", "call", "construct"))
            self.members[member] = self.members.get(member, 0) + n


def load_inventories(directory: Path) -> tuple[dict[str, Seen], list[dict]]:
    seen: dict[str, Seen] = {}
    metas: list[dict] = []
    for path in sorted(directory.glob("*.json")):
        data = json.loads(path.read_text(encoding="utf-8"))
        site = data.get("meta", {}).get("site") or path.stem
        metas.append({"site": site, **{k: data["meta"].get(k) for k in ("date", "chromium_version", "tracer_version", "interface_count")}})
        for name, entry in data.get("interfaces", {}).items():
            seen.setdefault(name, Seen(name)).add(site, entry)
    if not seen:
        raise SystemExit(f"no inventories found in {directory}")
    return seen, metas


# ---- assignment ------------------------------------------------------------


def idl_file(spec: str, interface: str) -> str:
    return f"idl/{spec}/{interface}.webidl"


def assign(seen: dict[str, Seen], rules: list[Rule], member_rules: list[MemberRule]) -> dict:
    assigned: list[dict] = []
    ecmascript: list[dict] = []
    unassigned: list[dict] = []
    for name in sorted(seen):
        s = seen[name]
        top = sorted(s.members.items(), key=lambda kv: (-kv[1], kv[0]))[:TOP_MEMBERS]
        base = {
            "interface": name,
            "seen_in": sorted(s.sites),
            "members_seen": len(s.members),
            "accesses": s.accesses,
            "realms": sorted(s.realms),
            "top_members": [m for m, _ in top],
        }
        rule = next((r for r in rules if r.matches(name)), None)
        if rule is None:
            unassigned.append({**base, "reason": "no rule in idl/phase-rules.toml matches"})
            continue
        if rule.phase == "unscheduled":
            unassigned.append({**base, "family": rule.spec, "wpt_dir": rule.wpt, "reason": f"family {rule.spec!r} is not scheduled in the roadmap" + (f": {rule.note}" if rule.note else "")})
            continue
        deferred = []
        for mr in member_rules:
            if mr.interface != name:
                continue
            for member in sorted(s.members):
                if mr.pattern.fullmatch(member):
                    deferred.append({"member": member, "phase": mr.phase, "crate": mr.crate, **({"note": mr.note} if mr.note else {})})
        if rule.kind == "ecmascript":
            ecmascript.append({**base, "phase": rule.phase, "crate": rule.crate, **({"note": rule.note} if rule.note else {})})
            continue
        entry = {
            "interface": name,
            "idl_file": idl_file(rule.spec, name),
            "spec": rule.spec,
            "phase": rule.phase,
            "crate": rule.crate,
            "wpt_dir": rule.wpt,
            "seen_in": base["seen_in"],
            "members_seen": base["members_seen"],
            "accesses": base["accesses"],
            "realms": base["realms"],
            "top_members": base["top_members"],
        }
        if deferred:
            entry["deferred_members"] = deferred
        if rule.note:
            entry["note"] = rule.note
        assigned.append(entry)
    return {"interface": assigned, "ecmascript": ecmascript, "unassigned": unassigned}


# ---- TOML emitter ----------------------------------------------------------


def toml_str(value: str) -> str:
    out = value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
    return f'"{out}"'


def toml_value(value) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return toml_str(value)
    if isinstance(value, list):
        if value and all(isinstance(v, dict) for v in value):
            return "[\n" + "".join(f"  {toml_inline_table(v)},\n" for v in value) + "]"
        return "[" + ", ".join(toml_value(v) for v in value) + "]"
    if isinstance(value, dict):
        return toml_inline_table(value)
    raise TypeError(f"cannot serialise {type(value).__name__}")


def toml_inline_table(table: dict) -> str:
    return "{ " + ", ".join(f"{k} = {toml_value(v)}" for k, v in table.items()) + " }"


def toml_table(header: str, table: dict, array: bool = False) -> str:
    lines = [f"[[{header}]]" if array else f"[{header}]"]
    for k, v in table.items():
        key = k if re.fullmatch(r"[A-Za-z0-9_-]+", k) else toml_str(k)
        lines.append(f"{key} = {toml_value(v)}")
    return "\n".join(lines) + "\n"


def render(result: dict, metas: list[dict], rules_data: dict, generated: str) -> str:
    by_phase: dict[str, int] = {}
    by_crate: dict[str, int] = {}
    for e in result["interface"] + result["ecmascript"]:
        by_phase[e["phase"]] = by_phase.get(e["phase"], 0) + 1
        by_crate[e["crate"]] = by_crate.get(e["crate"], 0) + 1
    by_site: dict[str, int] = {}
    for e in result["interface"] + result["ecmascript"] + result["unassigned"]:
        for site in e["seen_in"]:
            by_site[site] = by_site.get(site, 0) + 1
    total = len(result["interface"]) + len(result["ecmascript"]) + len(result["unassigned"])

    parts = [
        "# idl/MANIFEST.toml: every web-platform interface seen in the golden-site\n"
        "# API inventories (golden/inventory/*.json), mapped to the phase that ships\n"
        "# it, the owning crate, the WebIDL file it will live in and its WPT directory.\n"
        "#\n"
        "# GENERATED by idl/gen_manifest.py from golden/inventory/*.json and\n"
        "# idl/phase-rules.toml. Do not edit by hand: change the rules (or re-run the\n"
        "# tracer) and regenerate. `python3 idl/gen_manifest.py --check` fails when\n"
        "# this file is stale.\n"
        "#\n"
        "# `accesses` is the summed member access count across sites (a priority\n"
        "# signal, not a coverage metric); `members_seen` counts distinct members\n"
        "# touched; `realms` says where the interface was used (window, worker,\n"
        "# service_worker), which informs [Exposed]. `deferred_members` lists members\n"
        "# implemented in a later phase or another crate than the interface itself.\n"
        "# The WebIDL files themselves are copied from the specs in P1/P2, not now.\n\n",
        toml_table("meta", {
            "generated": generated,
            "generator": "idl/gen_manifest.py",
            "rules_file": "idl/phase-rules.toml",
            "rules_version": rules_data.get("version", 0),
            "inventories": metas,
        }),
        "\n",
        toml_table("stats", {
            "interfaces": total,
            "assigned": len(result["interface"]),
            "ecmascript": len(result["ecmascript"]),
            "unassigned": len(result["unassigned"]),
        }),
        toml_table("stats.by_phase", {p: by_phase[p] for p in PHASE_ORDER if p in by_phase}),
        toml_table("stats.by_crate", dict(sorted(by_crate.items(), key=lambda kv: (-kv[1], kv[0])))),
        toml_table("stats.by_site", dict(sorted(by_site.items()))),
        "\n# ---- WebIDL interfaces, grouped by phase ------------------------------------\n\n",
    ]
    for phase in PHASE_ORDER:
        entries = [e for e in result["interface"] if e["phase"] == phase]
        if not entries:
            continue
        parts.append(f"# {phase}: {rules_data.get('phases', {}).get(phase, '')} ({len(entries)} WebIDL interfaces)\n\n")
        for e in sorted(entries, key=lambda e: (e["crate"], e["spec"], e["interface"])):
            parts.append(toml_table("interface", e, array=True) + "\n")
    parts.append("# ---- ECMAScript intrinsics traced for engine planning (no WebIDL file) ------\n\n")
    for e in result["ecmascript"]:
        parts.append(toml_table("ecmascript", e, array=True) + "\n")
    parts.append("# ---- Unassigned: no rule matched, or the family is not scheduled -------------\n"
                 "# A human assigns these by adding or changing a [[rule]] in phase-rules.toml.\n\n")
    for e in sorted(result["unassigned"], key=lambda e: (-e["accesses"], e["interface"])):
        parts.append(toml_table("unassigned", e, array=True) + "\n")
    return "".join(parts)


# ---- main ------------------------------------------------------------------


def strip_generated(text: str) -> str:
    return re.sub(r'^generated = ".*"$', "", text, count=1, flags=re.M)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--check", action="store_true", help="verify idl/MANIFEST.toml is up to date instead of writing it")
    ap.add_argument("--inventory-dir", type=Path, default=INVENTORY_DIR)
    ap.add_argument("--rules", type=Path, default=RULES_FILE)
    ap.add_argument("--out", type=Path, default=MANIFEST_FILE)
    args = ap.parse_args(argv)

    rules, member_rules, rules_data = load_rules(args.rules)
    seen, metas = load_inventories(args.inventory_dir)
    result = assign(seen, rules, member_rules)
    generated = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    text = render(result, metas, rules_data, generated)
    # Sanity: the emitted TOML must parse back.
    parsed = tomllib.loads(text)
    assert parsed["stats"]["interfaces"] == len(seen), "entry count mismatch"

    if args.check:
        if not args.out.exists():
            print(f"{args.out} is missing", file=sys.stderr)
            return 1
        if strip_generated(args.out.read_text(encoding="utf-8")) != strip_generated(text):
            print(f"{args.out} is stale; run python3 idl/gen_manifest.py", file=sys.stderr)
            return 1
        print(f"{args.out} is up to date")
        return 0

    args.out.write_text(text, encoding="utf-8")
    s = parsed["stats"]
    phases = ", ".join(f"{p}={n}" for p, n in parsed["stats"]["by_phase"].items())
    print(f"wrote {args.out}: {s['interfaces']} interfaces ({s['assigned']} assigned, {s['ecmascript']} ecmascript, {s['unassigned']} unassigned); {phases}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
