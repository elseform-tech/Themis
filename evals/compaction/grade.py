"""Deterministic grading of probe answers (no model judge)."""

import re

UNKNOWN = re.compile(r"^\W*unknown\b", re.IGNORECASE)


def position(planted_at, records):
    """Third of the transcript where the fact was planted."""
    fraction = (planted_at - 1) / max(records, 1)
    return "early" if fraction < 1 / 3 else "middle" if fraction < 2 / 3 else "late"


def _matches(spec, answer):
    lowered = " ".join(answer.lower().split())
    if any(" ".join(item.lower().split()) in lowered for item in spec.get("any_of", [])):
        return True
    return bool(spec.get("regex")) and re.search(spec["regex"], answer, re.IGNORECASE) is not None


def grade(probe, answer):
    """Returns one verdict: correct, unknown, stale, wrong, invented, correct_unknown or error."""
    if answer is None:
        return "error"
    unknown = UNKNOWN.match(answer.strip()) is not None
    if probe.get("expect_unknown"):
        return "correct_unknown" if unknown else "invented"
    if unknown:
        return "unknown"
    if _matches(probe["expected"], answer):
        return "correct"
    if any(re.search(pattern, answer, re.IGNORECASE) for pattern in probe.get("stale", [])):
        return "stale"
    return "wrong"
