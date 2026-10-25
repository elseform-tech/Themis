"""The soak's job: a generated project of N small source files, deterministic per (seed, N).

Each `src/<name>.txt` holds a few facts (owner team, region, count, status) above some notes
filler, so reading every source fills the context and forces checkpoints. `FORMAT.md` says how
each output is written; `check.py` checks the facts. The two rules (owner line first; retired
items listed in the index but given no file) are only in the first chat message and never in
the project, so keeping them depends on the chat and its checkpoints.
"""

import json
import os
import random
import re

JOB = "items-v1"
DEFAULT_ITEMS = 40
DEFAULT_FILLER_CHARS = 4000
HERE = os.path.dirname(os.path.abspath(__file__))
PROMPTS = os.path.join(HERE, "prompts")

ADJECTIVES = ("amber", "brisk", "cobalt", "dusky", "ember", "fallow", "gilded", "hollow", "ivory", "jade",
              "keen", "lunar", "mossy", "nimble", "onyx", "pale", "quiet", "russet", "silver", "tawny",
              "umber", "vivid", "woolly", "young", "zesty")
NOUNS = ("falcon", "badger", "cedar", "delta", "egret", "fjord", "garnet", "heron", "inlet", "juniper",
         "kestrel", "lagoon", "marten", "nettle", "otter", "plover", "quarry", "raven", "sorrel", "thistle",
         "upland", "vole", "willow", "yarrow", "zephyr")
TEAMS = ("team-lynx", "team-orca", "team-wren", "team-ibex", "team-kite", "team-newt")
REGIONS = ("north", "south", "east", "west", "central")
STATUSES = ("active", "paused", "retired")
FILLER_WORDS = ("the", "ledger", "batch", "was", "moved", "after", "review", "of", "quarterly", "notes",
                "and", "a", "minor", "rename", "in", "storage", "with", "no", "change", "to", "its",
                "numbers", "while", "operators", "logged", "routine", "checks", "on", "schedule")


def prompt(file, **values):
    """A prompt file from `prompts/` with its `{{snake_case}}` placeholders filled."""
    with open(os.path.join(PROMPTS, f"{file}.md"), encoding="utf-8") as handle:
        text = handle.read()
    for key, value in values.items():
        text = text.replace("{{" + key + "}}", str(value))
    missing = re.findall(r"\{\{[a-z_]+\}\}", text)
    if missing:
        raise KeyError(f"{file}: unfilled {missing}")
    return text


def _filler(rng, chars):
    lines, size = [], 0
    while size < chars:
        words = [rng.choice(FILLER_WORDS) for _ in range(rng.randint(10, 18))]
        line = " ".join(words).capitalize() + "."
        lines.append(line)
        size += len(line) + 1
    return "\n".join(lines)


def spec_for(seed, items=DEFAULT_ITEMS):
    """The items and their facts for a seed: names unique, at least two of each status."""
    if items < 6:
        raise ValueError("the soak needs at least 6 items")
    rng = random.Random(f"soak:{JOB}:{seed}:{items}")
    pairs = [(a, n) for a in ADJECTIVES for n in NOUNS]
    rng.shuffle(pairs)
    names = sorted(f"{a}-{n}" for a, n in pairs[:items])
    statuses = ["active", "paused", "retired"] * 2 + [rng.choices(STATUSES, weights=(6, 3, 2))[0]
                                                      for _ in range(items - 6)]
    rng.shuffle(statuses)
    entries = [{"name": name, "owner": rng.choice(TEAMS), "region": rng.choice(REGIONS),
                "count": rng.randint(10, 999), "status": status} for name, status in zip(names, statuses)]
    return {"job": JOB, "seed": seed, "size": items, "items": entries}


def source_text(item, filler):
    return (f"name: {item['name']}\nowner: {item['owner']}\nregion: {item['region']}\n"
            f"count: {item['count']}\nstatus: {item['status']}\n\nNotes:\n{filler}\n")


def output_text(item, upper=False):
    """What `out/<name>.md` holds when written correctly (both rules kept)."""
    status = item["status"].upper() if upper else item["status"]
    return (f"owner: {item['owner']}\n# {item['name']}\n- region: {item['region']}\n"
            f"- count: {item['count']}\n- status: {status}\n")


def index_text(spec, upper=False):
    lines = [f"- {item['name']}: {item['status'].upper() if upper else item['status']}" for item in spec["items"]]
    return "\n".join(lines) + f"\ntotal: {total(spec)}\n"


def total(spec):
    return sum(item["count"] for item in spec["items"])


def generate(seed, project, items=DEFAULT_ITEMS, filler_chars=DEFAULT_FILLER_CHARS):
    """Writes the project and returns its spec (with `filler_chars`)."""
    spec = spec_for(seed, items)
    spec["filler_chars"] = filler_chars
    rng = random.Random(f"soak-filler:{seed}:{items}")
    os.makedirs(os.path.join(project, "src"), exist_ok=True)
    for item in spec["items"]:
        with open(os.path.join(project, "src", f"{item['name']}.txt"), "w", encoding="utf-8") as handle:
            handle.write(source_text(item, _filler(rng, filler_chars)))
    with open(os.path.join(project, "FORMAT.md"), "w", encoding="utf-8") as handle:
        handle.write(prompt("format"))
    with open(os.path.join(HERE, "check_template.py"), encoding="utf-8") as handle:
        check = handle.read()
    with open(os.path.join(project, "check.py"), "w", encoding="utf-8") as handle:
        handle.write(check)
    return spec


def messages(spec):
    """The chat's scripted messages: rules (before /goal), the goal, the midway follow-up and
    the recall questions (after reached), each with what a correct answer contains."""
    first = spec["items"][0]
    retired = sum(item["status"] == "retired" for item in spec["items"])
    return {
        "rules": prompt("rules"),
        "goal": prompt("goal"),
        "followup": prompt("followup"),
        "recall": [
            {"id": "early_fact", "text": prompt("recall-fact", name=first["name"]),
             "expect": rf"\b{first['count']}\b"},
            {"id": "rule", "text": prompt("recall-rule"), "expect": r"owner\s*:"},
            {"id": "count", "text": prompt("recall-count"), "expect": rf"\b{retired}\b"},
        ],
    }


def material_tokens(project):
    """≈ tokens of the project's text (bytes ÷ 4)."""
    size = 0
    for folder, _dirs, files in os.walk(project):
        if ".git" in folder.split(os.sep):
            continue
        size += sum(os.path.getsize(os.path.join(folder, name)) for name in files)
    return size // 4


def dump(spec, path):
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(spec, handle, indent=1)
