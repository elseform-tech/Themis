"""Deterministic grading of agent-mode answers (no model judge)."""

import re

from evals.compaction.grade import UNKNOWN

CITATION_LINE = re.compile(r"^\W*citations?\b.*$", re.IGNORECASE | re.MULTILINE)
QUOTES = str.maketrans({"“": '"', "”": '"', "‘": "'", "’": "'"})
MAX_ANSWER_CHARS = 800  # the prompt asks for one short sentence; a dump that names the value is not an answer

LEADS_UNKNOWN = re.compile(r"^\W*unknown\b|^\W*no\W*$", re.IGNORECASE)
NOT_FOUND = re.compile(
    r"\b(no (matches|mentions?|results?|records?|evidence|trace|such|information|details?)"
    r"|nothing (in|about|on|that)|not (found|present|appear|mentioned|stated|recorded|in the)"
    r"|never (mention|state|say|appear|record)|(did|does|do) ?n[o']?t (find|appear|see|contain|mention|say)"
    r"|could ?n[o']?t (find|locate)|cannot (find|locate))\b",
    re.IGNORECASE)
ASSERTION = re.compile(r"\b(was|is|are|were|equals?|=)\b", re.IGNORECASE)
QUOTED = re.compile(r"\"[^\"]*\"|`[^`]*`|'[^']{0,40}'")
CANDIDATE = re.compile(r"\d{2,}|\w+[_./:-]\w+")


def abstained(text):
    """True only for a real abstention: it leads with UNKNOWN, or its first sentence says the
    material has nothing, asserts no value before saying so, and names no concrete candidate."""
    if LEADS_UNKNOWN.match(text):
        return True
    first = re.split(r"(?<=[.!?])\s", text.strip(), maxsplit=1)[0]
    found = NOT_FOUND.search(first)
    if not found or ASSERTION.search(first[:found.start()]):
        return False
    return CANDIDATE.search(QUOTED.sub("", text)) is None


def body(answer):
    """The answer without its `Citation:` lines (citations must not satisfy or break the grader)."""
    return CITATION_LINE.sub("", answer).strip()


def normalize(text):
    return " ".join(text.translate(QUOTES).split())


def anchored(token):
    from .kit import anchored as make
    return make(token)


def hedged(probe, text, spans, pool):
    """Other planted values named in the answer, outside the matched/superseded spans and the question."""
    question = probe.get("question", "").lower()
    rest = text
    for start, end in sorted(spans, reverse=True):
        rest = rest[:start] + " " + rest[end:]
    extras = [t for t in pool if t not in question and re.search(anchored(t), rest, re.IGNORECASE)]
    return [t for t in extras if not any(t in other and t != other for other in extras)]


def grade(probe, answer, pool=()):
    """One verdict: correct, unknown, stale, wrong, hedged, invented, correct_unknown or error."""
    if answer is None:
        return "error"
    text = body(answer)
    if probe.get("expect_unknown"):
        return "correct_unknown" if abstained(text) else "invented"
    if UNKNOWN.match(text) or LEADS_UNKNOWN.match(text):
        return "unknown"
    if len(text) > MAX_ANSWER_CHARS:
        return "wrong"
    stale = [m for pattern in probe.get("stale", [])
             if (m := re.search(pattern, text, re.IGNORECASE))]
    if "exact" in probe:
        # Exact wording is required: case-sensitive, whitespace and quote-style normalised.
        if normalize(probe["exact"]) not in normalize(text):
            return "unknown" if abstained(text) else "wrong"
        text = normalize(text)
        found = text.find(normalize(probe["exact"]))
        spans = [(found, found + len(normalize(probe["exact"])))]
    else:
        expected = re.search(probe["expected"]["regex"], text, re.IGNORECASE | re.DOTALL)
        if expected and stale and min(m.start() for m in stale) < expected.start():
            return "stale"  # leads with the superseded value
        if not expected:
            if abstained(text):
                return "unknown"
            return "stale" if stale else "wrong"
        spans = [expected.span()] + [m.span() for m in stale]
    return "hedged" if hedged(probe, text, spans, pool) else "correct"
