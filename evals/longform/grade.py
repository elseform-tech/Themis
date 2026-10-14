"""Deterministic grading of agent-mode answers (no model judge)."""

import re

from evals.compaction.grade import UNKNOWN, abstains

CITATION_LINE = re.compile(r"^\W*citations?\b.*$", re.IGNORECASE | re.MULTILINE)
QUOTES = str.maketrans({"“": '"', "”": '"', "‘": "'", "’": "'"})


def body(answer):
    """The answer without its `Citation:` lines (citations must not satisfy or break the grader)."""
    return CITATION_LINE.sub("", answer).strip()


def normalize(text):
    return " ".join(text.translate(QUOTES).split())


MAX_ANSWER_CHARS = 800  # the prompt asks for one short sentence; a dump that names the value is not an answer


def grade(probe, answer):
    """One verdict: correct, unknown, stale, wrong, invented, correct_unknown or error."""
    if answer is None:
        return "error"
    text = body(answer)
    if probe.get("expect_unknown"):
        return "correct_unknown" if abstains(text) else "invented"
    if UNKNOWN.match(text):
        return "unknown"
    if len(text) > MAX_ANSWER_CHARS:
        return "wrong"
    if "exact" in probe:
        # Exact wording is required: case-sensitive, whitespace and quote-style normalised.
        return "correct" if normalize(probe["exact"]) in normalize(text) else (
            "unknown" if abstains(text) else "wrong")
    expected = re.search(probe["expected"]["regex"], text, re.IGNORECASE | re.DOTALL)
    stale = [m for pattern in probe.get("stale", [])
             if (m := re.search(pattern, text, re.IGNORECASE))]
    if expected and stale and min(m.start() for m in stale) < expected.start():
        return "stale"  # leads with the superseded value
    if expected:
        return "correct"
    if abstains(text):
        return "unknown"
    return "stale" if stale else "wrong"
