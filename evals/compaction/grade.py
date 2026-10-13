"""Deterministic grading of probe answers (no model judge)."""

import re

# "UNKNOWN", optionally followed by an explanation. "Unknown <value>" is not an abstention.
UNKNOWN = re.compile(
    r"^\W*unknown\W*$"
    r"|^\W*unknown\s*[.:;\u2014\u2013-]+\s*(the |this |no |not |there |it |i |nothing|neither|none|cannot|can't)",
    re.IGNORECASE)
# Plain-language abstentions.
ABSTAINS = re.compile(
    r"\b(not (been )?(mentioned|stated|specified|provided|discussed|said|given|recorded)"
    r"|(does|do|did|has|have) not (say|state|specify|mention|include|contain|provide|indicate)"
    r"|(doesn't|don't|didn't|hasn't|haven't) (say|state|specify|mention|include|contain|provide|indicate)"
    r"|no (information|record|mention|details?) (is |was |are |were )?(available|provided|given|about|on|of|in|regarding)"
    r"|cannot (be )?determine[d]?|can't (be )?determine[d]?|unable to determine)\b",
    re.IGNORECASE)


def abstains(answer):
    text = answer.strip()
    return UNKNOWN.match(text) is not None or ABSTAINS.search(text) is not None


def position(planted_at, records):
    """Third of the transcript where the fact was planted."""
    fraction = (planted_at - 1) / max(records, 1)
    return "early" if fraction < 1 / 3 else "middle" if fraction < 2 / 3 else "late"


def matches(spec, answer):
    lowered = " ".join(answer.lower().split())
    if any(" ".join(item.lower().split()) in lowered for item in spec.get("any_of", [])):
        return True
    return bool(spec.get("regex")) and re.search(spec["regex"], answer, re.IGNORECASE) is not None


def grade(probe, answer):
    """Returns one verdict: correct, unknown, stale, wrong, invented, correct_unknown or error."""
    if answer is None:
        return "error"
    if probe.get("expect_unknown"):
        return "correct_unknown" if abstains(answer) else "invented"
    if UNKNOWN.match(answer.strip()):
        return "unknown"
    if matches(probe["expected"], answer):
        return "correct"
    if ABSTAINS.search(answer):
        return "unknown"
    if any(re.search(pattern, answer, re.IGNORECASE) for pattern in probe.get("stale", [])):
        return "stale"
    return "wrong"
