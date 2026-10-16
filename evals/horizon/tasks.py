"""Long-horizon tasks: jobs big enough to cross several context checkpoints.

Each job is run in three arms: `no-plan` (update_plan withheld), `plan-offered` (the tool and
its instruction, as in the app) and `plan-asked` (the prompt also asks for a plan, so plan
persistence can be measured even when the model would not plan by itself). Checks are
deterministic: the files on disk, details from early in the task named in the final answer,
and redone work.
"""

import os
import random
import re

from evals.tools.tasks import read, write

HEROES = ["Odile", "Bram", "Sunniva", "Teodor", "Yara", "Caspian", "Wren", "Ilsa", "Mattias", "Petra",
          "Anouk", "Leopold", "Maren", "Tobias", "Signe", "Elio", "Freya", "Rasmus", "Liv", "Konrad",
          "Ines", "Halvard", "Saga", "Vidar"]
MODULES = 16
CITIES = ["Ghent", "Tromso", "Bergamo", "Aarhus", "Cork", "Tartu", "Lviv", "Porto", "Graz", "Uppsala",
          "Bilbao", "Kaunas", "Lille", "Turku", "Brno", "Malmo"]
OWNER_RULE = "owner: heron"
ASK_PLAN = " Keep a short plan with update_plan and keep it current as you work."


def setup_stories(project):
    """24 stories of ~5 KB about one named hero each; with a 12k-token budget the job crosses
    several checkpoints. Summaries need reading (a script cannot write them)."""
    rng = random.Random(41)
    places = ["harbour", "mill", "orchard", "lighthouse", "market", "quarry", "bridge", "abbey"]
    deeds = ["mended a broken net", "counted the lanterns", "carried salt to the cellar", "argued with the miller",
             "found a lost key", "repainted the gate", "fed the goats", "read the old ledger", "climbed the tower",
             "waited for the ferry"]
    for number, hero in enumerate(HEROES, 1):
        sentences = [f"{hero} walked to the {rng.choice(places)} at dawn."]
        while len(" ".join(sentences)) < 5000:
            sentences.append(f"Later {rng.choice(['that day', 'in the evening', 'after the rain', 'before supper'])} "
                             f"{hero} {rng.choice(deeds)} near the {rng.choice(places)}.")
        write(project, f"stories/story-{number:02d}.txt", " ".join(sentences) + "\n")
    os.makedirs(os.path.join(project, ".themis/context/eval-thread"), exist_ok=True)


def module_functions(number):
    rng = random.Random(1000 + number)
    verbs = ["load", "parse", "merge", "score", "render", "flush", "rank", "split", "trim", "audit"]
    nouns = ["ledger", "batch", "token", "frame", "queue", "shard", "badge", "route", "quota", "cache"]
    names = []
    while len(names) < 3 + number % 3:
        name = f"{rng.choice(verbs)}_{rng.choice(nouns)}_{number:02d}"
        if name not in names:
            names.append(name)
    return names


def setup_modules(project):
    """16 modules, each a ~4.5 KB docstring about one city followed by 3-5 functions. The
    summary line needs the docstring read (no search finds the city without knowing it)."""
    rng = random.Random(77)
    sights = ["the old harbour", "a covered market", "the river bridges", "a hill fort", "the tram depot",
              "a botanical garden", "the cathedral square", "a ferry terminal"]
    for number, city in enumerate(CITIES, 1):
        story = ["This module has a long history.", f"It was first written for the office in {city}."]
        while len(" ".join(story)) < 4500:
            story.append(f"Visitors to {city} {rng.choice(['often', 'rarely', 'always', 'sometimes'])} "
                         f"remember {rng.choice(sights)}.")
        parts = ['"""' + " ".join(story) + '"""\n\n']
        for name in module_functions(number):
            parts.append(f"def {name}(value):\n    return value\n\n")
        write(project, f"src/mod-{number:02d}.py", "".join(parts))
    os.makedirs(os.path.join(project, ".themis/context/eval-thread"), exist_ok=True)


# Both jobs ask for reading file by file: a generator script would finish without ever filling
# the context, and the eval is about what survives checkpoints.
STORIES_PROMPT = (
    "The stories folder has story-01.txt to story-24.txt. Read each story and write report.txt with one line "
    "per story, in file order and each story exactly once, in the form "
    "'story-01.txt: <one-sentence summary that names its main character>'. When report.txt is complete, "
    "finish with one sentence that names the main character of story-01.txt and the number of stories.")

MODULES_PROMPT = (
    f"For each of src/mod-01.py to src/mod-{MODULES:02d}.py, write notes/mod-NN.md (same number). Rule for every "
    f"notes file: its first line is exactly '{OWNER_RULE}', followed by the module's function names, one per "
    "line, in file order, and a last line 'summary: <one sentence on what the module docstring is about>'. "
    "Read each module with read_file and write each notes file yourself; do not use scripts to extract or "
    "generate the notes. Do not change anything in src/. Then write notes/SUMMARY.md with one line per module "
    "in order, 'mod-NN: <number of functions>', and a last line 'total: <sum>'. Finish with one sentence that "
    "repeats the rule for notes files and gives the total.")


def check_stories(project, report):
    text = read(project, "report.txt") or ""
    got = [line.strip() for line in text.strip().splitlines() if line.strip()]
    problems = []
    if len(got) != len(HEROES):
        problems.append(f"{len(got)} lines")
    if len(set(got)) != len(got):
        problems.append("duplicates")
    for number, (line, hero) in enumerate(zip(got, HEROES), 1):
        if not line.startswith(f"story-{number:02d}.txt:") or hero.lower() not in line.lower():
            problems.append(f"line {number} {line[:40]!r}")
            break
    return not problems, "; ".join(problems)


def measure_stories(project, report):
    answer = report.get("answer", "")
    return {"early_detail": bool(re.search(r"\bodile\b", answer, re.I)),
            "count_recalled": bool(re.search(r"\b24\b|twenty-four", answer, re.I)),
            "rereads": rereads(report, r"story-\d\d\.txt")}


def check_modules(project, report):
    problems = []
    for number in range(1, MODULES + 1):
        text = read(project, f"notes/mod-{number:02d}.md")
        if text is None:
            problems.append(f"mod-{number:02d}.md missing")
            break
        lines = [line.strip().strip("-* `") for line in text.strip().splitlines() if line.strip()]
        if not lines or lines[0] != OWNER_RULE:
            problems.append(f"mod-{number:02d}.md first line {lines[:1]!r}")
            break
        if lines[1:-1] != module_functions(number):
            problems.append(f"mod-{number:02d}.md functions")
            break
        if not (lines[-1].lower().startswith("summary:") and CITIES[number - 1].lower() in lines[-1].lower()):
            problems.append(f"mod-{number:02d}.md summary {lines[-1][:40]!r}")
            break
    summary = [line.strip() for line in (read(project, "notes/SUMMARY.md") or "").strip().splitlines() if line.strip()]
    counts = [len(module_functions(n)) for n in range(1, MODULES + 1)]
    expected = [f"mod-{n:02d}: {c}" for n, c in enumerate(counts, 1)] + [f"total: {sum(counts)}"]
    if summary != expected:
        problems.append("SUMMARY.md")
    for number in range(1, MODULES + 1):
        original = read(project, f"src/mod-{number:02d}.py") or ""
        if f"def {module_functions(number)[0]}(value):" not in original:
            problems.append(f"src/mod-{number:02d}.py changed")
            break
    return not problems, "; ".join(problems)


def measure_modules(project, report):
    answer = report.get("answer", "")
    total = sum(len(module_functions(n)) for n in range(1, MODULES + 1))
    # How many notes files keep the rule given only at the start (late files test retention).
    kept = sum((read(project, f"notes/mod-{n:02d}.md") or "").lstrip().startswith(OWNER_RULE)
               for n in range(1, MODULES + 1))
    return {"early_detail": OWNER_RULE in answer.lower(), "count_recalled": str(total) in answer,
            "rule_kept_files": kept, "rereads": rereads(report, r"src/mod-\d\d\.py")}


def rereads(report, pattern):
    """read_file calls on a source file the agent had already read: work redone after forgetting."""
    seen, again = set(), 0
    for call in report.get("calls", []):
        if call["tool"] != "read_file":
            continue
        match = re.search(pattern, call.get("summary", ""))
        if match:
            again += match.group(0) in seen
            seen.add(match.group(0))
    return again


JOBS = [
    {"name": "stories", "setup": setup_stories, "prompt": STORIES_PROMPT, "check": check_stories,
     "measure": measure_stories},
    {"name": "modules", "setup": setup_modules, "prompt": MODULES_PROMPT, "check": check_modules,
     "measure": measure_modules},
]
COMMON = {"evidence": ".themis/context/eval-thread", "budget": 12_000, "recent_messages": 4, "max_turns": 90,
          "timeout": 2400}


def arms(job):
    base = dict(COMMON, **job, tool="update_plan", job=job["name"])
    return [dict(base, name=f"{job['name']}-no-plan", arm="no-plan", offer_plan=False),
            dict(base, name=f"{job['name']}-plan-offered", arm="plan-offered", offer_plan=True),
            dict(base, name=f"{job['name']}-plan-asked", arm="plan-asked", offer_plan=True,
                 prompt=job["prompt"] + ASK_PLAN)]


TASKS = [task for job in JOBS for task in arms(job)]
