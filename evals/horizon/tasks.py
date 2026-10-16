"""Long-horizon tasks: jobs big enough to cross several context checkpoints.

Arms: `no-notes` (the notes tool withheld), `notes-offered` (the tool and its instruction,
as in the app), `notes-asked` (the prompt also asks for notes, so persistence can be measured
even when the model would not keep notes by itself) and `notes-advice` (notes offered, and the
prompt carries the former read-one-write-one advice, to measure whether it helps). The
`modules-turns` job splits the modules job over four chat turns with a progress question; the
`ports` job needs every file read before anything can be written. Checks are deterministic: the
files on disk, details from early in the task named in the final answer, and redone work.
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
# The read-one-write-one system instruction removed on 2026-10-16, as a prompt suffix.
ADVICE = (" Context is limited and older reads are summarised away at checkpoints. For jobs over many files or"
          " items, finish and save each item's result (write its output) before reading the next few; do not"
          " read everything first.")
SERVICES = 14
ASK_NOTES = (" Keep notes with the notes tool: the goal and rules, each item's status, and the results you"
             " will need at the end, updated as you finish each item.")


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


def service_port(number):
    """Ports with four collisions (three pairs and one triple) among 14 services."""
    return [8080, 9000, 8443, 7000, 9000, 6500, 8080, 5432, 7100, 6500, 8443, 9300, 6500, 5000][number - 1]


def setup_ports(project):
    """14 service files of ~4.5 KB; the port sits after ~3 KB of notes, so it needs reading.
    No item can be finished alone: conflicts are only known once every file has been read."""
    rng = random.Random(53)
    teams = ["billing", "search", "ingest", "alerts", "reports", "auth", "export"]
    for number in range(1, SERVICES + 1):
        lines = [f"# svc-{number:02d} service definition", f"team: {rng.choice(teams)}"]
        while len("\n".join(lines)) < 3000:
            lines.append(f"# note: the {rng.choice(teams)} team reviewed this file {rng.choice(['twice', 'once', 'last week'])}.")
        lines.append(f"port: {service_port(number)}")
        while len("\n".join(lines)) < 4500:
            lines.append(f"# history: moved from the {rng.choice(teams)} cluster.")
        write(project, f"services/svc-{number:02d}.yaml", "\n".join(lines) + "\n")
    os.makedirs(os.path.join(project, ".themis/context/eval-thread"), exist_ok=True)


def port_conflicts():
    users = {}
    for number in range(1, SERVICES + 1):
        users.setdefault(service_port(number), []).append(f"svc-{number:02d}")
    return {port: names for port, names in sorted(users.items()) if len(names) > 1}


# Both jobs ask for reading file by file: a generator script would finish without ever filling
# the context, and the eval is about what survives checkpoints.
STORIES_PROMPT = (
    "The stories folder has story-01.txt to story-24.txt. Read each story and write report.txt with one line "
    "per story, in file order and each story exactly once, in the form "
    "'story-01.txt: <one-sentence summary that names its main character>'. When report.txt is complete, "
    "finish with one sentence that names the main character of story-01.txt and the number of stories.")

MODULES_PROMPT = (
    f"For each of src/mod-01.py to src/mod-{MODULES:02d}.py, write cards/mod-NN.md (same number). Rule for every "
    f"card file: its first line is exactly '{OWNER_RULE}', followed by the module's function names, one per "
    "line, in file order, and a last line 'summary: <one sentence on what the module docstring is about>'. "
    "Read each module with read_file and write each card file yourself; do not use scripts to extract or "
    "generate the cards. Do not change anything in src/. Then write cards/SUMMARY.md with one line per module "
    "in order, 'mod-NN: <number of functions>', and a last line 'total: <sum>'. Finish with one sentence that "
    "repeats the rule for card files and gives the total.")


PORTS_PROMPT = (
    f"The services folder has svc-01.yaml to svc-{SERVICES:02d}.yaml, each with one 'port:' line. Find every port "
    "used by more than one service. Read each file with read_file; do not use search, grep or scripts to find the "
    "ports. Write conflicts.md with one line per shared port, in ascending port order, in the form "
    "'port <number>: svc-AA, svc-BB' (services in file order). When conflicts.md is complete, finish with one "
    "sentence that gives the number of shared ports and the port of svc-01.")


def check_ports(project, report):
    got = [line.strip() for line in (read(project, "conflicts.md") or "").strip().splitlines() if line.strip()]
    expected = [f"port {port}: {', '.join(names)}" for port, names in port_conflicts().items()]
    return (True, "") if got == expected else (False, f"conflicts.md {got[:2]!r}")


def measure_ports(project, report):
    answer = report.get("answer", "")
    return {"early_detail": str(service_port(1)) in answer,
            "count_recalled": bool(re.search(rf"\b{len(port_conflicts())}\b|four", answer, re.I)),
            "rereads": rereads(report, r"svc-\d\d\.yaml")}


FREE_PROMPT = MODULES_PROMPT.replace(
    "Read each module with read_file and write each card file yourself; do not use scripts to extract or "
    "generate the cards. ", "")


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
        text = read(project, f"cards/mod-{number:02d}.md")
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
    summary = [line.strip() for line in (read(project, "cards/SUMMARY.md") or "").strip().splitlines() if line.strip()]
    counts = [len(module_functions(n)) for n in range(1, MODULES + 1)]
    expected = [f"mod-{n:02d}: {c}" for n, c in enumerate(counts, 1)] + [f"total: {sum(counts)}"]
    if summary != expected:
        # A right list with a wrong last line is an addition slip, not lost work.
        problems.append("SUMMARY.md total" if summary[:-1] == expected[:-1] else "SUMMARY.md")
    for number in range(1, MODULES + 1):
        original = read(project, f"src/mod-{number:02d}.py") or ""
        if f"def {module_functions(number)[0]}(value):" not in original:
            problems.append(f"src/mod-{number:02d}.py changed")
            break
    return not problems, "; ".join(problems)


def measure_modules(project, report):
    answer = report.get("answer", "")
    total = sum(len(module_functions(n)) for n in range(1, MODULES + 1))
    # How many card files keep the rule given only at the start (late files test retention).
    kept = sum((read(project, f"cards/mod-{n:02d}.md") or "").lstrip().startswith(OWNER_RULE)
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


ARMS = ("no-notes", "notes-offered", "notes-asked", "notes-advice")
JOBS = [
    {"name": "stories", "setup": setup_stories, "prompt": STORIES_PROMPT, "check": check_stories,
     "measure": measure_stories, "arms": ARMS[:3]},
    {"name": "modules", "setup": setup_modules, "prompt": MODULES_PROMPT, "check": check_modules,
     "measure": measure_modules, "arms": ARMS},
    {"name": "ports", "setup": setup_ports, "prompt": PORTS_PROMPT, "check": check_ports,
     "measure": measure_ports, "arms": ARMS[:3]},
    # The modules job with the agent untied: no ban on scripts and YOLO-style approvals (the
    # OS sandbox still applies), to see what the memory-test restrictions cost.
    {"name": "modules-free", "setup": setup_modules, "prompt": FREE_PROMPT, "check": check_modules,
     "measure": measure_modules, "arms": ARMS[:2], "allow_all": True},
]
COMMON = {"evidence": ".themis/context/eval-thread", "budget": 12_000, "recent_messages": 4, "max_turns": 90,
          "timeout": 2400}


def arm_prompt(prompt, arm):
    return prompt + {"notes-asked": ASK_NOTES, "notes-advice": ADVICE}.get(arm, "")


def arms(job):
    base = {k: v for k, v in dict(COMMON, **job, tool="notes", job=job["name"]).items() if k != "arms"}
    return [dict(base, name=f"{job['name']}-{arm}", arm=arm, offer_notes=arm != "no-notes",
                 prompt=arm_prompt(job["prompt"], arm)) for arm in job["arms"]]


# The modules job over four turns: half, a progress question (no work), the rest, then a
# question about the start. Progress and the early rule must survive turns and checkpoints.
TURNS_PROMPT = MODULES_PROMPT.replace(
    f"For each of src/mod-01.py to src/mod-{MODULES:02d}.py",
    f"We will do this job over several messages. For each of src/mod-01.py to src/mod-{MODULES:02d}.py",
) + " For now, do mod-01 to mod-08 only, then stop and wait for me."
TURNS_FOLLOWUPS = [
    "Progress check: how many modules have card files so far, which ones are left, and what is the "
    "function count so far? Do not continue the job yet.",
    "Continue the job: finish mod-09 to mod-16 and write cards/SUMMARY.md.",
    "Remind me: what rule did I give for the first line of every card file, and what is the total?",
]


def check_turns(project, report):
    passed, why = check_modules(project, report)
    turns = report.get("turns", [])
    if len(turns) < 4:
        return False, f"{len(turns)} turns; {why}"
    progress = turns[1]["answer"]
    # Eight of sixteen done, and the count so far matches mod-01..08.
    so_far = sum(len(module_functions(n)) for n in range(1, 9))
    progress_ok = re.search(r"\b8\b|eight", progress, re.I) is not None and str(so_far) in progress
    total = sum(len(module_functions(n)) for n in range(1, MODULES + 1))
    final = turns[3]["answer"]
    recall_ok = OWNER_RULE in final.lower() and str(total) in final
    problems = [p for p in [why, "" if progress_ok else f"progress {progress[:60]!r}",
                            "" if recall_ok else f"final {final[:60]!r}"] if p]
    return passed and progress_ok and recall_ok, "; ".join(problems)


def measure_turns(project, report):
    turns = report.get("turns", [])
    measured = measure_modules(project, dict(report, answer=turns[-1]["answer"] if turns else ""))
    progress = turns[1]["answer"] if len(turns) > 1 else ""
    so_far = sum(len(module_functions(n)) for n in range(1, 9))
    measured["progress_reported"] = bool(re.search(r"\b8\b|eight", progress, re.I)) and str(so_far) in progress
    return measured


TURNS_TASKS = [dict(COMMON, setup=setup_modules, prompt=arm_prompt(TURNS_PROMPT, arm),
                    followups=TURNS_FOLLOWUPS, check=check_turns, measure=measure_turns, tool="notes",
                    job="modules-turns", name=f"modules-turns-{arm}", arm=arm, offer_notes=arm != "no-notes",
                    timeout=4800)
               for arm in ARMS]

TASKS = [task for job in JOBS for task in arms(job)] + TURNS_TASKS
