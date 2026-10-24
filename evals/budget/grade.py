"""The three messages, and grading by hidden tests per stage and per desk.

Hidden tests run against a copy of the project in a sandbox with `evals.hard.sheet.hidden` (no
network, no reads or writes under /Users or /Volumes on macOS); the agent's own tests run the
same way with `evals.hard.sheet.own_tests`.
"""

import os
import re

from evals.hard import sheet

from .generate import hidden_tests

PROMPTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "prompts")
STAGES = ("stage1", "stage2", "stage3")
PREFIXES = {"stage1": "test_s1_", "stage2": "test_s2_", "stage3": "test_s3_"}


def fill(template, **values):
    """`{{snake_case}}` placeholders filled as `prompts::fill` does: every one, each value used."""
    names = set(re.findall(r"\{\{([a-z_]+)\}\}", template))
    if names != set(values):
        raise ValueError(f"placeholders {sorted(names)} do not match values {sorted(values)}")
    for name, value in values.items():
        template = template.replace("{{" + name + "}}", str(value))
    return template


def prompt(name, **values):
    with open(os.path.join(PROMPTS, name + ".md"), encoding="utf-8") as handle:
        return fill(handle.read(), **values)


def messages(spec):
    """The three messages of one chat: implement, change and extend, report."""
    return [prompt("stage1", desk_count=len(spec.desks)),
            prompt("stage2", tier=spec.changed_tier, new_min=spec.new_min),
            prompt("stage3")]


def stages(result, desk_names):
    """(stage -> passed, desk -> passed) from a hidden-suite result. A suite that did not run
    (a timeout, an import failure) passes nothing."""
    failed = result.get("failed") or []
    if result.get("run", 0) == 0 or {"timeout", "import"} & set(failed):
        return {s: False for s in STAGES}, {d: False for d in desk_names}
    by_stage = {s: not any(name.startswith(PREFIXES[s]) for name in failed) for s in STAGES}
    by_desk = {d: not any(name in (f"test_s1__{d}", f"test_s2__{d}") for name in failed) for d in desk_names}
    return by_stage, by_desk


def grade(project, spec):
    """Grades a finished project: every stage's hidden tests and the agent's own tests."""
    result = sheet.hidden(project, hidden_tests(spec))
    by_stage, by_desk = stages(result, [d.name for d in spec.desks])
    own_passed, own_count = sheet.own_tests(project)
    found = []
    if not all(by_stage.values()):
        failing = [s for s, ok in by_stage.items() if not ok]
        found.append(f"stages failing: {', '.join(failing)} ({', '.join((result.get('failed') or [])[:6])})")
    if own_count == 0:
        found.append("no own tests")
    elif not own_passed:
        found.append("own tests fail")
    return {"passed": not found, "why": "; ".join(found), "stages": by_stage,
            "stages_passed": sum(by_stage.values()), "desks_passed": sum(by_desk.values()),
            "desks": len(by_desk), "failed_desks": sorted(d for d, ok in by_desk.items() if not ok)[:20],
            "own_tests": {"passed": own_passed, "count": own_count},
            "hidden_details": [d[-300:] for d in (result.get("details") or [])[:2]]}
