"""Provider and run error classes (copied from `evals/longform/report.py`).

Raw error text is reduced to a class and an HTTP status; nothing else is kept (no bodies, no
paths), so reports never carry provider responses.
"""

import re

# Request failures, not agent failures: a run lost to one of these is infrastructure.
INFRASTRUCTURE_CLASSES = ("provider_request", "provider_response", "timeout")


def classify_error(text):
    """Reduces raw error text to a class and an HTTP status; nothing else is kept (no bodies, no paths)."""
    if not text:
        return None
    lowered = text.lower()
    status = re.search(r"\b(?:http|status|code)\D{0,6}([45]\d\d)\b|\b([45]\d\d)\b(?= (?:too many|bad|unauthor|forbidden|not found|internal|service))", lowered)
    status = next((g for g in status.groups() if g), None) if status else None
    for name, needles in (("timeout", ("timed out", "timeout")),
                          ("provider_response", ("response format error", "decode", "truncated")),
                          ("provider_request", ("llm request failed", "provider", "rate limit", "connection", "request failed")),
                          ("tool_loop", ("repeated unchanged tool results",)),
                          ("stopped", ("stopped by you",)),
                          ("turn_limit", ("max turns", "turn limit", "maximum number of turns"))):
        if any(n in lowered for n in needles):
            return f"{name}:{status}" if status else name
    return "other"


def is_infrastructure(error_class):
    return bool(error_class) and error_class.startswith(INFRASTRUCTURE_CLASSES)
