"""The soak's job: a generated project of N prose memos, deterministic per (seed, N).

Each `src/<name>.txt` is a short memo in varied wording. The graded facts are never written as
`key: value` lines; each has to be read and resolved:

- the **owner** is the team that holds the item now, after a hand-over told in a sentence (and
  sometimes a hand-over that was proposed and dropped);
- the **status** (`active`, `paused`, `retired`) is implied by events ("switched off for good",
  "put on hold", "back in service"), often after an earlier, different state;
- the **count** of records is written in words, as a sum of two places, as a corrected figure, or
  as a number next to a figure that belongs to something else;
- the **purpose** keyword must appear in the item's one-line summary; a sibling with another
  purpose is mentioned too;
- the **since** year (asked for by the midway follow-up) is the year the current status began,
  among other years.

Background paragraphs (other teams, years and numbers) pad every memo, so reading all of them
fills a small context many times over, and a regex over the files gets most items wrong
(`test_harness.ScriptShortcutTests`). `FORMAT.md` says how each output is written; `check.py`
checks only structure and the index total (by hash), so it reveals no fact. The two rules
(owner line first; retired items listed in the index but given no file) are only in the first
chat message.
"""

import hashlib
import json
import os
import random
import re

JOB = "items-v2"
DEFAULT_ITEMS = 120
DEFAULT_FILLER_CHARS = 6000
HERE = os.path.dirname(os.path.abspath(__file__))
PROMPTS = os.path.join(HERE, "prompts")

ADJECTIVES = ("amber", "brisk", "cobalt", "dusky", "ember", "fallow", "gilded", "hollow", "ivory", "jade",
              "keen", "lunar", "mossy", "nimble", "onyx", "pale", "quiet", "russet", "silver", "tawny",
              "umber", "vivid", "woolly", "young", "zesty")
NOUNS = ("falcon", "badger", "cedar", "delta", "egret", "fjord", "garnet", "heron", "inlet", "juniper",
         "kestrel", "lagoon", "marten", "nettle", "otter", "plover", "quarry", "raven", "sorrel", "thistle",
         "upland", "vole", "willow", "yarrow", "zephyr")
TEAMS = ("team-lynx", "team-orca", "team-wren", "team-ibex", "team-kite", "team-newt", "team-moth", "team-hare")
STATUSES = ("active", "paused", "retired")
PURPOSES = ("invoices", "payroll", "shipments", "telemetry", "forecasts", "tickets", "contracts", "inventory",
            "backups", "permits", "timesheets", "refunds", "manifests", "warranties", "audits", "rosters")
DEPARTMENTS = ("finance", "the warehouse", "field operations", "customer care", "legal", "the regional office",
               "procurement", "the night shift")
PLACES = ("north depot", "south depot", "east annex", "west annex", "main hall", "river site")
CHECK_SALT = "soak-items-v2"

_ONES = ("zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
         "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen")
_TENS = ("", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety")


def words(number):
    """British-style number words for 0..9999: 412 -> "four hundred and twelve"."""
    if number < 20:
        return _ONES[number]
    if number < 100:
        tens, ones = divmod(number, 10)
        return _TENS[tens] + (f"-{_ONES[ones]}" if ones else "")
    if number < 1000:
        hundreds, rest = divmod(number, 100)
        return f"{_ONES[hundreds]} hundred" + (f" and {words(rest)}" if rest else "")
    thousands, rest = divmod(number, 1000)
    tail = "" if not rest else (f" and {words(rest)}" if rest < 100 else f" {words(rest)}")
    return f"{words(thousands)} thousand{tail}"


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


# --- facts --------------------------------------------------------------------------------------

def spec_for(seed, items=DEFAULT_ITEMS):
    """The items and their facts for a seed: names unique, at least two of each status, and the
    phrasing choices (`forms`) that the memo text is built from."""
    if items < 6:
        raise ValueError("the soak needs at least 6 items")
    rng = random.Random(f"soak:{JOB}:{seed}:{items}")
    pairs = [(a, n) for a in ADJECTIVES for n in NOUNS]
    rng.shuffle(pairs)
    names = sorted(f"{a}-{n}" for a, n in pairs[:items])
    statuses = ["active", "paused", "retired"] * 2 + [rng.choices(STATUSES, weights=(6, 3, 2))[0]
                                                      for _ in range(items - 6)]
    rng.shuffle(statuses)
    entries = []
    for name, status in zip(names, statuses):
        first, owner = rng.sample(TEAMS, 2)
        handed = rng.random() < 0.75
        since = rng.randint(2016, 2024)
        purpose, sibling_purpose = rng.sample(PURPOSES, 2)
        entries.append({
            "name": name, "owner": owner if handed else first, "status": status,
            "count": rng.randint(40, 1900), "purpose": purpose, "since": since,
            "forms": {"owner": rng.randrange(6) if handed else 6 + rng.randrange(3), "first_owner": first,
                      "dropped_team": rng.choice([t for t in TEAMS if t not in (first, owner)]),
                      "dropped": rng.random() < 0.4, "status": rng.randrange(3),
                      "earlier": rng.randint(2012, since - 1), "count": rng.randrange(4),
                      "split": rng.randint(15, 85) / 100, "wrong": rng.randint(30, 400),
                      "other": rng.randint(100, 9000), "places": rng.sample(PLACES, 2),
                      "sibling": rng.choice([n for n in names if n != name]), "sibling_purpose": sibling_purpose,
                      "department": rng.choice(DEPARTMENTS), "order": rng.random()},
        })
    return {"job": JOB, "seed": seed, "size": items, "items": entries}


def _owner_sentences(item):
    f = item["forms"]
    first, owner, dropped = f["first_owner"], item["owner"], f["dropped_team"]
    form = f["owner"]
    sentences = [
        f"It was first set up by {first}, and in {f['earlier']} it was handed over to {owner}, who look after it today.",
        f"{owner} took it over from {first} a while ago; {first} only answers questions about its early history.",
        f"Ownership passed from {first} to {owner} after a reorganisation, and that arrangement still stands.",
        f"For years {first} ran it. These days, though, it belongs to {owner}.",
        f"{first} built it, but responsibility moved to {owner} when {first} changed focus.",
        f"The current keepers are {owner}; it came to them from {first}.",
        f"{owner} has looked after it from the very start.",
        f"It has always been the responsibility of {owner}, ever since it was built.",
        f"Nobody but {owner} has ever owned it.",
    ][form]
    out = [sentences]
    if f["dropped"]:
        out.append(f"A proposal to move it to {dropped} was discussed in {f['earlier']}, but it was dropped and "
                   f"nothing changed.")
    return out


def _status_sentence(item):
    f, year, earlier = item["forms"], item["since"], item["forms"]["earlier"]
    options = {
        "active": [f"It was paused in {earlier} for repairs but has been running again since {year}.",
                   f"After a short hold in {earlier}, it came back into service in {year} and is in daily use.",
                   f"It has worked without interruption since it went live in {year}."],
        "paused": [f"It ran normally until {year}, when it was put on hold pending a budget review; it has not "
                   f"resumed, but nobody has switched it off either.",
                   f"Work on it was suspended in {year} and is still waiting for a decision.",
                   f"It was busy until {earlier}; since {year} it has sat idle on hold, kept ready to restart."],
        "retired": [f"It was paused in {earlier} and then shut down permanently in {year}.",
                    f"In {year} it was decommissioned and its hardware was recycled.",
                    f"It was switched off for good in {year}; there are no plans to bring it back."],
    }
    return options[item["status"]][f["status"]]


def _count_sentence(item):
    f, count = item["forms"], item["count"]
    form = f["count"]
    if form == 0:
        return f"Right now it holds {words(count)} records."
    if form == 1:
        here = max(1, round(count * f["split"]))
        return (f"Its records are split between two sites: {here} are kept at the {f['places'][0]} and the other "
                f"{count - here} at the {f['places'][1]}.")
    if form == 2:
        wrong = count + f["wrong"] if count + f["wrong"] != count else count + 1
        return (f"The first audit logged {wrong} records, but that figure double-counted copies; the corrected "
                f"total is {count}.")
    return (f"It holds {count} records. The archive of {f['other']} records in the same room belongs to "
            f"another system entirely.")


def _purpose_sentences(item):
    f = item["forms"]
    return [f"Its job is to keep the {item['purpose']} for {f['department']}.",
            f"It is not to be confused with {f['sibling']}, which deals with {f['sibling_purpose']}."]


BACKGROUND = (
    "In {year}, {team} borrowed the room next to it for a week to test new shelving.",
    "A visitor survey in {year} counted {n} people walking through the {place}.",
    "The {place} was repainted in {year}; the work took {small} days and {team} supervised it.",
    "Somebody once suggested moving everything to the {place}, but the idea never got past a meeting in {year}.",
    "{team} keeps a spare key for the {place}, mostly out of habit.",
    "The heating in the {place} failed twice in {year}, and {n} complaints were filed before it was fixed.",
    "A training day in {year} covered fire drills, ladder safety and the {place} car park.",
    "{team} won the office quiz in {year} with {small} points to spare.",
    "Around {n} boxes of old brochures were cleared out of the {place} in {year}.",
    "The coffee machine near the {place} was replaced in {year} after {small} years of service.",
    "Records from {year} mention {team} lending {small} chairs to a neighbouring group.",
    "A storm in {year} closed the {place} for {small} days; nothing inside was damaged.",
    # Distractors in the facts' own vocabulary, about other things.
    "The old printer in the {place} was replaced in {year}; it was switched off for good after that.",
    "The canteen project was put on hold in {year}, and it has not resumed.",
    "The lift in the {place} has been running again since {year}.",
    "The {place} archive next door holds {n} records of its own.",
    "{team} took over the {place} cleaning rota from {team2} in {year}.",
    "A sister system that kept the {purpose} was decommissioned in {year}.",
    "In {year} the visitor log was handed over to {team}, who still keep it.",
    "The first count of the {place} shelves came to {n}, later corrected to {n2}.",
)


def _background(rng, chars):
    sentences, size = [], 0
    while size < chars:
        team, team2 = rng.sample(TEAMS, 2)
        text = rng.choice(BACKGROUND).format(year=rng.randint(2005, 2025), team=team, team2=team2,
                                             n=rng.randint(12, 4000), n2=rng.randint(12, 4000),
                                             place=rng.choice(PLACES), small=rng.randint(2, 19),
                                             purpose=rng.choice(PURPOSES))
        sentences.append(text)
        size += len(text) + 1
    paragraphs, step = [], 4
    for start in range(0, len(sentences), step):
        paragraphs.append(" ".join(sentences[start:start + step]))
    return paragraphs


def source_text(item, rng, filler_chars):
    """The memo: facts spread over a few paragraphs, between background paragraphs."""
    facts = [_status_sentence(item), _count_sentence(item), *_owner_sentences(item), *_purpose_sentences(item)]
    rng.shuffle(facts)
    background = _background(rng, filler_chars)
    split = max(1, int(len(background) * item["forms"]["order"]))
    middle = [" ".join(facts[:3]), " ".join(facts[3:])]
    body = background[:split] + middle[:1] + background[split:split + 1] + middle[1:] + background[split + 1:]
    return f"Memo about {item['name']}\n\n" + "\n\n".join(body) + "\n"


# --- outputs ------------------------------------------------------------------------------------

def output_text(item, since=False):
    """A correct `out/<name>.md` (both rules kept; with the follow-up's line when `since`)."""
    lines = [f"owner: {item['owner']}", f"# {item['name']}", f"- count: {item['count']}",
             f"- status: {item['status']}"]
    if since:
        lines.append(f"- since: {item['since']}")
    lines.append(f"- summary: Keeps the {item['purpose']} for {item['forms']['department']}.")
    return "\n".join(lines) + "\n"


def index_text(spec):
    lines = [f"- {item['name']}: {item['status']}" for item in spec["items"]]
    return "\n".join(lines) + f"\ntotal: {total(spec)}\n"


def total(spec):
    return sum(item["count"] for item in spec["items"])


def total_digest(value):
    return hashlib.sha256(f"{CHECK_SALT}:{value}".encode()).hexdigest()


def checker_text(spec):
    """`check.py` for this project: structure only, and the index total by its hash."""
    with open(os.path.join(HERE, "check_template.py"), encoding="utf-8") as handle:
        template = handle.read()
    return template.replace("{{total_digest}}", total_digest(total(spec))).replace("{{salt}}", CHECK_SALT)


def generate(seed, project, items=DEFAULT_ITEMS, filler_chars=DEFAULT_FILLER_CHARS):
    """Writes the project and returns its spec (with `filler_chars`)."""
    spec = spec_for(seed, items)
    spec["filler_chars"] = filler_chars
    rng = random.Random(f"soak-memo:{JOB}:{seed}:{items}")
    os.makedirs(os.path.join(project, "src"), exist_ok=True)
    for item in spec["items"]:
        with open(os.path.join(project, "src", f"{item['name']}.txt"), "w", encoding="utf-8") as handle:
            handle.write(source_text(item, rng, filler_chars))
    with open(os.path.join(project, "FORMAT.md"), "w", encoding="utf-8") as handle:
        handle.write(prompt("format"))
    with open(os.path.join(project, "check.py"), "w", encoding="utf-8") as handle:
        handle.write(checker_text(spec))
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


# --- grading the outputs ------------------------------------------------------------------------

def _field(text, field):
    match = re.search(rf"^\s*-\s*{field}:\s*(.+?)\s*$", text or "", re.MULTILINE | re.IGNORECASE)
    return match.group(1) if match else None


def item_correct(item, text):
    """Whether an item's output states its count, status and purpose right (the owner line is
    graded with the rules; the since line with the follow-up)."""
    if text is None:
        return False
    count, status, summary = _field(text, "count"), _field(text, "status"), _field(text, "summary") or ""
    stem = item["purpose"].rstrip("s").lower()
    return (count is not None and re.sub(r"[,\s]", "", count) == str(item["count"])
            and (status or "").lower() == item["status"] and stem in summary.lower())


def owner_correct(item, text):
    return (text or "").splitlines()[:1] == [f"owner: {item['owner']}"]


def since_correct(item, text):
    return (_field(text, "since") or "").strip() == str(item["since"])


def index_statuses(spec, text):
    """Share of items whose status in `out/INDEX.md` is right."""
    rows = dict(re.findall(r"^\s*-\s*([a-z]+-[a-z]+):\s*(\S+)\s*$", text or "", re.MULTILINE))
    right = sum(rows.get(item["name"], "").lower() == item["status"] for item in spec["items"])
    return right / len(spec["items"])


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
