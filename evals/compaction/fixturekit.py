"""Deterministic helpers for authoring synthetic compaction fixtures.

Fixtures are generated once by `gen_cases.py` and committed; this module only
keeps the generator readable. Nothing here is imported by the harness at run time.
"""

import json
import random

from .grade import matches

NOUNS = "ledger widget gateway cursor tenant shard bucket session ticket payload digest router planner cache batch socket queue index record schema fixture token window handler adapter".split()
VERBS = "build parse merge flush resolve render collect encode verify rotate expire normalize hydrate sample dispatch compact".split()
ADJ = "pending stale primary shared lazy strict partial rolling nested opaque sparse bounded".split()


class Transcript:
    """Collects transcript records and the planted-fact bookkeeping."""

    def __init__(self, seed):
        self.rng = random.Random(seed)
        self.records = []
        self.boundaries = []
        self._call = 0
        self.scale = 1

    def _add(self, record):
        self.records.append(record)
        return len(self.records)  # 1-based record number

    def user(self, text):
        return self._add({"role": "user", "content": text})

    def asst(self, text):
        return self._add({"role": "assistant", "content": text})

    def call(self, name, arguments, output, text=""):
        """One assistant tool call plus its result; returns the result's record number."""
        self._call += 1
        cid = f"call_{self._call:03d}"
        self._add({"role": "assistant", "content": text,
                   "tool_calls": [{"id": cid, "name": name, "arguments": arguments}]})
        return self._add({"role": "tool",
                          "results": [{"id": cid, "name": name, "output": output}]})

    def prose(self, sentences):
        return prose(self.rng, sentences * self.scale)

    def boundary(self):
        """Mark a compaction point at the current end (after a completed turn)."""
        self.boundaries.append(len(self.records))

    def write(self, path):
        with open(path, "w", encoding="utf-8") as handle:
            for record in self.records:
                handle.write(json.dumps(record, ensure_ascii=False) + "\n")


def ident(rng, kind="snake"):
    parts = [rng.choice(VERBS), rng.choice(ADJ), rng.choice(NOUNS)]
    return "_".join(parts) if kind == "snake" else "".join(p.title() for p in parts)


def python_module(rng, functions=8):
    """Plausible-looking Python source with unique identifiers."""
    lines = ['"""Internal helpers."""', "", "import logging", "", "log = logging.getLogger(__name__)", ""]
    for _ in range(functions):
        name = ident(rng)
        a, b = rng.choice(NOUNS), rng.choice(NOUNS)
        lines += [
            f"def {name}({a}, {b}=None, *, limit={rng.randint(3, 99)}):",
            f'    """{rng.choice(VERBS).title()} the {a} using the {b} when present."""',
            f"    if {a} is None:",
            f"        log.debug('skipping {name}: no {a}')",
            "        return []",
            f"    items = [item for item in {a} if item is not None][:limit]",
            f"    if {b} is not None:",
            f"        items = sorted(items, key=lambda item: getattr(item, '{rng.choice(NOUNS)}', 0))",
            f"    log.info('{name} produced %d items', len(items))",
            "    return items",
            "",
        ]
    return "\n".join(lines)


def rust_module(rng, functions=8):
    lines = ["//! Internal helpers.", "", "use std::collections::HashMap;", ""]
    for _ in range(functions):
        name = ident(rng)
        a = rng.choice(NOUNS)
        lines += [
            f"/// {rng.choice(VERBS).title()} one {a} from the input.",
            f"pub fn {name}(input: &str, limit: usize) -> Vec<String> {{",
            "    let mut seen: HashMap<&str, usize> = HashMap::new();",
            "    let mut out = Vec::new();",
            "    for part in input.split(',').map(str::trim).filter(|p| !p.is_empty()) {",
            "        let count = seen.entry(part).or_insert(0);",
            "        *count += 1;",
            f"        if *count == 1 && out.len() < limit {{ out.push(format!(\"{{}}:{a}\", part)); }}",
            "    }",
            "    out",
            "}",
            "",
        ]
    return "\n".join(lines)


def log_lines(rng, count=40, prefix="INFO"):
    out = []
    for i in range(count):
        out.append(f"2026-10-0{rng.randint(1, 9)}T{rng.randint(10, 23)}:{rng.randint(10, 59)}:{rng.randint(10, 59)}Z {prefix} {rng.choice(NOUNS)}.{rng.choice(VERBS)} id={rng.randint(10000, 99999)} took={rng.randint(2, 480)}ms")
    return "\n".join(out)


SENTENCES = [
    "The {a} {v}s every {n} before anyone notices the {b}.",
    "Nobody trusted the {a} after the {b} went quiet.",
    "She counted the {n} twice, then {v}d the {a} by hand.",
    "A {a} {n} drifted past, and the {b} did not look up.",
    "He kept the {a} close, because the {b} had a long memory.",
]


def prose(rng, sentences=8):
    words = NOUNS
    out = []
    for _ in range(sentences):
        out.append(rng.choice(SENTENCES).format(a=rng.choice(ADJ) + " " + rng.choice(words), b=rng.choice(words), n=rng.choice(words), v=rng.choice(VERBS)))
    return " ".join(out)


def finish(case_dir, name, kind, transcript, probes, next_action, description):
    """Writes transcript.jsonl, probes.json and case.json, validating probe records."""
    import os
    os.makedirs(case_dir, exist_ok=True)
    total = len(transcript.records)
    if not transcript.boundaries or transcript.boundaries[-1] != len(transcript.records):
        transcript.boundary()  # the final compaction covers the whole conversation
    ends = transcript.boundaries
    assert ends and ends == sorted(set(ends)) and ends[-1] <= total, (name, ends, total)
    for probe in probes:
        assert 1 <= probe["planted_at"] <= total, (name, probe["id"])
        if probe.get("expect_unknown"):
            continue
        # The planted record must itself state the fact the probe expects.
        text = json.dumps(transcript.records[probe["planted_at"] - 1], ensure_ascii=False)
        spec = {"regex": probe["needle"]} if probe.get("needle") else probe["expected"]
        assert matches(spec, text), (name, probe["id"], probe["planted_at"])
    transcript.write(os.path.join(case_dir, "transcript.jsonl"))
    with open(os.path.join(case_dir, "probes.json"), "w", encoding="utf-8") as handle:
        json.dump(probes, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    with open(os.path.join(case_dir, "case.json"), "w", encoding="utf-8") as handle:
        json.dump({"name": name, "kind": kind, "description": description,
                   "suites": ["core"], "records": total, "chunks": ends, "next_action": next_action}, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
