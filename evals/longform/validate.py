"""Citation validation against the saved snapshots (the originals the agent could read)."""

import json
import os
import re

from evals.longform.kit import flatten

MAX_CITATIONS = 3  # more cited records than this is shotgun citing and is invalid
CITATION = re.compile(r"(snapshot-\d+\.jsonl)[^\d\n]{0,40}?(\d+(?:\s*[-,]\s*\d+)*)", re.IGNORECASE)


def parse_citations(answer):
    """Returns [(snapshot file name, record number)] named in the answer, ranges expanded (<= 6 wide)."""
    cited = []
    for name, numbers in CITATION.findall(answer or ""):
        for part in re.split(r"\s*,\s*", numbers):
            if "-" in part:
                low, high = (int(x) for x in re.split(r"\s*-\s*", part, maxsplit=1))
                cited += [(name, n) for n in range(low, min(high, low + 5) + 1)]
            else:
                cited.append((name, int(part)))
    return list(dict.fromkeys(cited))


def record_text(line):
    """Decoded original text of a snapshot record, or None for navigation/recovery copies.

    Tool results marked `recovery` are dropped; JSON-looking string values are decoded once so
    a fact inside an offloaded envelope is still found.
    """
    record = json.loads(line)
    if record.get("evidence_kind") in ("navigation", "recovery"):
        return None
    results = record.get("message_type", {}).get("ToolResult") if isinstance(record.get("message_type"), dict) else None
    if results:
        record["message_type"]["ToolResult"] = [r for r in results if r.get("evidence_kind") != "recovery"]
    text = flatten(record)
    extra = []
    for value in re.findall(r"^[\[{].*[\]}]$", text, re.MULTILINE):
        try:
            extra.append(flatten(json.loads(value)))
        except ValueError:
            pass
    return "\n".join([text] + extra)


def validate(evidence_dir, probe, answer):
    """{'present': any snapshot citation, 'valid': one of at most MAX_CITATIONS cited records is an original stating the fact}."""
    cited = parse_citations(answer)
    result = {"present": bool(cited), "valid": False, "count": len(cited), "over_cap": len(cited) > MAX_CITATIONS,
              "cited": [f"{n}:{r}" for n, r in cited[:8]]}
    if result["over_cap"]:
        return result
    cache = {}
    for name, number in cited:
        path = os.path.join(evidence_dir, os.path.basename(name))
        if path not in cache:
            try:
                with open(path, encoding="utf-8") as handle:
                    cache[path] = handle.read().splitlines()
            except OSError:
                cache[path] = []
        lines = cache[path]
        if not 1 <= number <= len(lines):
            continue
        try:
            text = record_text(lines[number - 1])
        except ValueError:
            continue
        if text is not None and re.search(probe["evidence"], text, re.IGNORECASE | re.DOTALL):
            result["valid"] = True
            break
    return result


def context_location(messages, probe):
    """Where the fact is in the post-compaction working messages: 'checkpoint', 'tail' or None."""
    found = None
    for index, message in enumerate(messages):
        if message.get("role") == "System":
            continue
        if re.search(probe["evidence"], flatten(message), re.IGNORECASE | re.DOTALL):
            if str(message.get("content", "")).startswith("Earlier context checkpoint"):
                found = found or "checkpoint"
            else:
                return "tail"
    return found
