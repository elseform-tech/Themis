"""Deterministic generator of the budget eval's project: a fictional ledger settlement service.

`generate(size, seed, project)` writes the project the agent receives (source skeletons, a
`docs/` spec with one page per desk, data files) and returns the `Spec` that the grader, the
reference solution and the messages are built from. Every rule a desk has (fee rate, rounding,
minimum fee, rejection floor and error codes, line order, hold days, calendar, tier, code)
is drawn from the seed and written only on that desk's own page, so it cannot be guessed.

Sizes are measured as bytes / 4 over everything the agent may read (`material_tokens`).
"""

import dataclasses
import datetime
import json
import os
import random
import string

from .reference import engine

SIZES = {"small": 6, "medium": 30, "large": 90}
# Target tokens of material per size, and the band the self-tests assert (±20%).
TARGET_TOKENS = {"small": 30_000, "medium": 150_000, "large": 450_000}
BAND = 0.2
ROUNDINGS = ("half_up", "half_even", "down", "up")
ORDERS = ("amount_desc", "amount_asc", "account", "id_desc", "date")
TIERS = ("A", "B", "C")
# The day the stage-3 hidden report test runs at, and the window of entry dates.
START = datetime.date(2026, 3, 2)

ADJECTIVES = ("amber", "basalt", "cobalt", "dusky", "ember", "fallow", "garnet", "hollow", "ivory",
              "juniper", "kestrel", "lumen", "marble", "nimbus", "obsidian", "pewter", "quartz",
              "russet", "sable", "tidal", "umber", "vesper", "willow", "zephyr")
NOUNS = ("quay", "harbor", "foundry", "arcade", "beacon", "cistern", "delta", "estuary", "fjord",
         "gantry", "heath", "inlet", "jetty", "keel", "lantern", "meadow", "narrows", "orchard",
         "pier", "reach", "spire", "terrace", "wharf", "yard")
PEOPLE = ("Ines Varga", "Tomasz Okafor", "Lena Morrow", "Ravi Castellanos", "Odile Fenwick",
          "Kofi Lindqvist", "Mara Delacroix", "Yusuf Brannigan", "Hana Ostrova", "Pell Achterberg",
          "Sunniva Ruiz", "Dario Whitlock")
PLACES = ("Port Calder", "Merrowgate", "Ostrand", "Vellmar", "Kessick Bay", "Thornhollow",
          "Alder Reach", "Saltmere", "Brindlefold", "Caskwater")

ROUNDING_TEXT = {
    "half_up": "rounded to the nearest cent, with exact halves rounded up",
    "half_even": "rounded to the nearest cent, with exact halves rounded to the even cent",
    "down": "always rounded down to the cent (any fraction of a cent is dropped)",
    "up": "always rounded up to the next whole cent when there is any fraction of a cent",
}
ORDER_TEXT = {
    "amount_desc": "largest amount first; equal amounts by entry id",
    "amount_asc": "smallest amount first; equal amounts by entry id",
    "account": "by account, then by entry id",
    "id_desc": "by entry id, highest first",
    "date": "by entry date, oldest first; the same date by entry id",
}
CALENDAR_TEXT = {
    "business": "business days (Saturdays and Sundays do not count)",
    "calendar": "calendar days (every day counts, weekends included)",
}


@dataclasses.dataclass
class Desk:
    name: str
    title: str
    code: str
    currency: str
    bps: int
    rounding: str
    min_fee: int
    floor: int
    floor_error: int
    currency_error: int
    order: str
    tier: str
    hold_days: int
    calendar: str
    region: str
    owner: str

    def rules(self, min_fee=None):
        """The engine's RULES for this desk, optionally with another minimum fee."""
        return {"code": self.code, "currency": self.currency, "bps": self.bps, "rounding": self.rounding,
                "min_fee": self.min_fee if min_fee is None else min_fee, "floor": self.floor,
                "floor_error": self.floor_error, "currency_error": self.currency_error,
                "order": self.order, "hold_days": self.hold_days, "calendar": self.calendar}


@dataclasses.dataclass
class Spec:
    size: str
    seed: int
    desks: list
    changed_tier: str
    new_min: int
    report_today: str

    def final_rules(self, desk):
        """Rules after message 2: the changed tier's desks take the new minimum fee."""
        return desk.rules(self.new_min if desk.tier == self.changed_tier else None)


def _words(rng, count):
    pairs = [(a, n) for a in ADJECTIVES for n in NOUNS]
    rng.shuffle(pairs)
    return pairs[:count]


def make_spec(size, seed):
    rng = random.Random(f"budget-spec-{size}-{seed}")
    count = SIZES[size]
    codes, desks = set(), []
    currencies = ["".join(rng.choice(string.ascii_uppercase) for _ in range(3)) for _ in range(4)]
    for adjective, noun in _words(rng, count):
        code = "".join(rng.choice(string.ascii_uppercase) for _ in range(3))
        while code in codes:
            code = "".join(rng.choice(string.ascii_uppercase) for _ in range(3))
        codes.add(code)
        # Odd rates that are not multiples of 5, so exact half cents exist for every rate.
        bps = rng.choice([b for b in range(11, 98) if b % 2 and b % 5])
        min_fee = rng.randrange(20, 250)
        errors = rng.sample(range(100, 1000), 2)
        desks.append(Desk(
            name=f"{adjective}_{noun}", title=f"{adjective.title()} {noun.title()}", code=code,
            currency=rng.choice(currencies), bps=bps, rounding=rng.choice(ROUNDINGS), min_fee=min_fee,
            floor=rng.randrange(min_fee + 300, 2500), floor_error=errors[0], currency_error=errors[1],
            order=rng.choice(ORDERS), tier=rng.choice(TIERS), hold_days=rng.randrange(0, 7),
            calendar=rng.choice(sorted(CALENDAR_TEXT)), region=rng.choice(PLACES), owner=rng.choice(PEOPLE)))
    changed = rng.choice(TIERS)
    # At least one desk inside the changed tier and one outside it, at every size.
    desks[0].tier = changed
    desks[1].tier = next(t for t in TIERS if t != changed)
    return Spec(size=size, seed=seed, desks=desks, changed_tier=changed, new_min=rng.randrange(300, 600),
                report_today=(START + datetime.timedelta(days=rng.randrange(8, 14))).isoformat())


# ---------------------------------------------------------------------------------------------
# Entries


def _entry(rng, desk, ids, amount, currency=None, account=None, day=None):
    ident = f"T{rng.randrange(10000, 99999)}"
    while ident in ids:
        ident = f"T{rng.randrange(10000, 99999)}"
    ids.add(ident)
    return {"id": ident, "account": account or "AC-" + "".join(rng.choice("BCDFGHJKLMNP") for _ in range(3)),
            "amount": amount, "currency": currency or desk.currency,
            "date": (day or START + datetime.timedelta(days=rng.randrange(0, 12))).isoformat()}


def _foreign(desk):
    return "XXX" if desk.currency != "XXX" else "YYY"


def _half_amounts(desk, low, count, parity):
    """Amounts >= low whose fee is an exact half cent, with an even or odd whole part."""
    found, amount = [], low
    while len(found) < count:
        whole, rest = divmod(amount * desk.bps, 10000)
        if rest == 5000 and whole % 2 == parity:
            found.append(amount)
        amount += 1
    return found


def stage1_entries(desk, spec, rng):
    """Entries whose settlement checks every stage-1 rule. For a desk whose minimum fee changes
    in message 2, every accepted fee stays above both minimums, so the change cannot touch them."""
    ids, entries = set(), []
    # Above every minimum that applies, so the rate and the rounding decide these fees.
    minimum = max(desk.min_fee, spec.new_min) if desk.tier == spec.changed_tier else desk.min_fee
    low = max(desk.floor, (minimum + 2) * 10000 // desk.bps + 1)
    for parity in (0, 1):
        for amount in _half_amounts(desk, low + rng.randrange(0, 5000), 2, parity):
            entries.append(_entry(rng, desk, ids, amount))
    for _ in range(4):
        entries.append(_entry(rng, desk, ids, rng.randrange(low, low + 400_000)))
    tie = rng.randrange(low, low + 90_000)
    entries.append(_entry(rng, desk, ids, tie))
    entries.append(_entry(rng, desk, ids, tie, account=entries[0]["account"], day=START))
    entries.append(_entry(rng, desk, ids, rng.randrange(low, low + 9000), day=START))
    if desk.tier != spec.changed_tier:
        # The minimum fee binds (any rate up to 1% would charge less than it).
        top = (desk.min_fee - 1) * 100
        if top >= desk.floor:
            entries.append(_entry(rng, desk, ids, rng.randrange(desk.floor, top + 1)))
    entries.append(_entry(rng, desk, ids, desk.floor - rng.randrange(1, 200)))
    entries.append(_entry(rng, desk, ids, desk.floor + 5000, currency=_foreign(desk)))
    entries.append(_entry(rng, desk, ids, max(1, desk.floor - 1), currency=_foreign(desk)))
    rng.shuffle(entries)
    return entries


def stage2_entries(desk, spec, rng):
    """Entries for message 2: the minimum fee (changed for one tier, unchanged for the others)
    and holds, mixed with rejected entries that holds must leave out."""
    ids, fees, hold = set(), [], []
    minimum = spec.new_min if desk.tier == spec.changed_tier else desk.min_fee
    top = (minimum - 1) * 100  # below this any rate up to 1% charges less than the minimum
    if top >= desk.floor:
        for _ in range(3):
            fees.append(_entry(rng, desk, ids, rng.randrange(desk.floor, top + 1)))
    for offset in range(-3, 9):
        day = datetime.date.fromisoformat(spec.report_today) - datetime.timedelta(
            days=desk.hold_days + offset)
        hold.append(_entry(rng, desk, ids, rng.randrange(desk.floor, desk.floor + 200_000), day=day))
    hold.append(_entry(rng, desk, ids, desk.floor - 1, day=START))
    hold.append(_entry(rng, desk, ids, desk.floor + 700, currency=_foreign(desk), day=START))
    rng.shuffle(hold)
    return fees, hold


def report_entries(desk, rng):
    ids = set()
    rows = [_entry(rng, desk, ids, rng.randrange(desk.floor, desk.floor + 300_000)) for _ in range(6)]
    rows.append(_entry(rng, desk, ids, desk.floor - 1))
    rows.append(_entry(rng, desk, ids, desk.floor + 10, currency=_foreign(desk)))
    rng.shuffle(rows)
    return rows


# ---------------------------------------------------------------------------------------------
# Hidden tests


HIDDEN = r'''import contextlib, csv, importlib, io, json, os, subprocess, sys, tempfile, unittest

CASES = json.loads(__CASES__)


def desk(name):
    return importlib.import_module("ledger.desks." + name)


def plain(value):
    """Lists, dicts, str and int only, so tuples or subclasses compare as their JSON form."""
    return json.loads(json.dumps(value))


class Stage1(unittest.TestCase):
    pass


class Stage2(unittest.TestCase):
    pass


def _stage1(case):
    def test(self):
        module = desk(case["name"])
        self.assertEqual(plain(module.settle([dict(e) for e in case["entries"]])), case["expected"])
    return test


def _stage2(case):
    def test(self):
        module = desk(case["name"])
        if case["fee_entries"]:
            self.assertEqual(plain(module.settle([dict(e) for e in case["fee_entries"]])), case["fees"])
        self.assertEqual(plain(module.holds([dict(e) for e in case["hold_entries"]], case["today"])),
                         case["holds"])
    return test


for _case in CASES["desks"]:
    setattr(Stage1, "test_s1__" + _case["name"], _stage1(_case["stage1"]))
    setattr(Stage2, "test_s2__" + _case["name"], _stage2(_case["stage2"]))


def _money(cents):
    return f"{cents // 100}.{cents % 100:02d}"


class Stage3(unittest.TestCase):
    """The report, checked against this project's own desk functions: its job is to put every
    desk together correctly, while the desks' rules are graded by stages 1 and 2."""

    def run_report(self, folder, today):
        return subprocess.run([sys.executable, "-m", "ledger", "report", folder, "--today", today],
                              capture_output=True, text=True, timeout=120)

    def test_s3_report(self):
        folder = tempfile.mkdtemp()
        for name, rows in CASES["report"]["files"].items():
            with open(os.path.join(folder, name + ".csv"), "w", newline="") as handle:
                writer = csv.DictWriter(handle, ["id", "account", "amount", "currency", "date"])
                writer.writeheader()
                writer.writerows(rows)
        today, expected, totals = CASES["report"]["today"], [], [0, 0, 0, 0, 0]
        with contextlib.redirect_stdout(io.StringIO()):
            for name, code in sorted(CASES["report"]["codes"].items(), key=lambda item: item[1]):
                rows = [dict(r, amount=int(r["amount"])) for r in CASES["report"]["files"].get(name, [])]
                settled, held = desk(name).settle(rows), desk(name).holds(rows, today)
                counts = [len(settled["lines"]), len(settled["errors"]), sum(l["fee"] for l in settled["lines"]),
                          len(held["released"]), len(held["held"])]
                totals = [a + b for a, b in zip(totals, counts)]
                expected.append(f"{code} lines={counts[0]} rejected={counts[1]} fees={_money(counts[2])} "
                                f"released={counts[3]} held={counts[4]}")
        expected.append(f"TOTAL desks={len(expected)} lines={totals[0]} rejected={totals[1]} "
                        f"fees={_money(totals[2])} released={totals[3]} held={totals[4]}")
        done = self.run_report(folder, today)
        self.assertEqual(done.returncode, 0, done.stderr[-400:])
        self.assertEqual(done.stdout.strip().splitlines(), expected)

    def test_s3_bad_date(self):
        done = self.run_report(tempfile.mkdtemp(), "2026-13-40")
        self.assertEqual(done.returncode, 2)
        self.assertTrue(done.stderr.strip())
        self.assertEqual(done.stdout, "")
'''


def hidden_cases(spec):
    rng = random.Random(f"budget-hidden-{spec.size}-{spec.seed}")
    desks = []
    for desk in spec.desks:
        entries = stage1_entries(desk, spec, rng)
        fee_entries, hold_entries = stage2_entries(desk, spec, rng)
        rules = spec.final_rules(desk)
        desks.append({"name": desk.name, "stage1": {
            "name": desk.name, "entries": entries, "expected": engine.settle(desk.rules(), entries)},
            "stage2": {"name": desk.name, "fee_entries": fee_entries,
                       "fees": engine.settle(rules, fee_entries) if fee_entries else None,
                       "hold_entries": hold_entries, "today": spec.report_today,
                       "holds": engine.holds(rules, hold_entries, spec.report_today)}})
    files = {d.name: report_entries(d, rng) for d in spec.desks}
    # One desk has no data file: the report counts it as having no entries.
    files.pop(spec.desks[-1].name)
    return {"desks": desks, "report": {"today": spec.report_today, "files": files,
                                       "codes": {d.name: d.code for d in spec.desks}}}


def hidden_tests(spec):
    return HIDDEN.replace("__CASES__", repr(json.dumps(hidden_cases(spec), separators=(",", ":"))))


# ---------------------------------------------------------------------------------------------
# The project the agent receives

OVERVIEW = """# Ledger settlement service

The service settles the day's entries for each trading desk. There are {count} desks; each one
is a module in `ledger/desks/` (the list is `ledger.desks.DESKS`) and has its own page in
`docs/desks/`. The rules below are shared by every desk. Each desk's own values (its code,
currency, fee rate, rounding, minimum fee, rejection floor, error numbers, line order, hold
period and calendar) are only on its own page.

## Entries

An entry is a dict with these keys:

| Key | Type | Meaning |
| --- | --- | --- |
| `id` | str | Unique entry id such as `T40213`. Ids compare as strings. |
| `account` | str | Account such as `AC-BKD`. |
| `amount` | int | Amount in cents, always positive. |
| `currency` | str | Three-letter currency code. |
| `date` | str | Entry date, `YYYY-MM-DD`. |

`data/<desk>.csv` holds sample entries with a header row `id,account,amount,currency,date`.

## Settlement: `settle(entries)`

Each desk module has `settle(entries)`, which returns `{{"lines": [...], "errors": [...]}}`:

- An entry in a currency other than the desk's settlement currency is rejected with the desk's
  currency error. Otherwise an entry whose amount is below the desk's floor is rejected with
  the desk's floor error. Rejected entries produce no line.
- Error codes are the desk code, a hyphen and the error number: `ABC-417`.
- `errors` is a list of `{{"id": ..., "code": ...}}`, sorted by entry id.
- Every accepted entry produces one line `{{"id", "account", "amount", "fee", "net"}}`, where
  `fee` is in whole cents and `net` is `amount - fee`.
- The fee is the desk's rate in basis points (1 basis point is 1/10000) of the amount, rounded
  to whole cents as the desk's page says, and never less than the desk's minimum fee.
- Lines are listed in the desk's line order.

Rounding names used on desk pages:

- *half up*: to the nearest cent, exact halves up.
- *half even*: to the nearest cent, exact halves to the even cent.
- *down*: drop any fraction of a cent.
- *up*: any fraction of a cent counts as a whole cent.

## Holds

Each desk holds settled funds for a number of days after the entry date, counted in calendar
days or in business days (Monday to Friday) as its page says. The release date is the entry
date moved forward by that many counted days; with a hold of 0 days it is the entry date
itself. Only entries the desk would accept are held or released.

## History and proposals

Desk pages keep a history of retired rates and rejected proposals for the record. Only the
current rules section of a page is in force.
"""

INTRO = (
    "The {title} desk settles entries booked through the {region} clearing window.",
    "It was set up by {owner}'s team after the consolidation of the regional ledgers.",
    "Most of its volume arrives in the early batch, and the remainder in the late correction run.",
    "Counterparties of the {title} desk are mostly mid-sized brokers and a handful of custodians.",
    "Questions about the desk go to the settlement operations rota first, then to {owner}.",
    "The desk shares its reconciliation tooling with the other desks of its tier.",
    "Its entries are reviewed weekly by the controls group in {place}.",
    "The desk keeps no positions of its own; it only settles what its counterparties book.",
)
FILLER = (
    "Operators confirm the batch totals against the clearing statement before the window closes, "
    "and any break larger than the tolerance agreed with the counterparty is escalated to {owner}.",
    "The late correction run in {place} replays entries once their senders have amended them; "
    "amended entries keep their original id, so downstream systems can match them.",
    "Account names are case-sensitive in the clearing statement but operators usually write them "
    "in upper case; the desk never rewrites an account it received.",
    "When a counterparty disputes a fee, the desk recomputes it from the entry and the rules in "
    "force on the entry date and answers within {days} working days.",
    "Statements are archived for {years} years in the {place} records office, after which only the "
    "monthly summaries are kept.",
    "The desk's cut-off is published in the operations calendar; entries arriving after it are "
    "booked with the next day's date by the sender, not by the desk.",
    "A quarterly review compares the desk's fee income with the forecast prepared by {owner} and "
    "notes any counterparty whose volume moved by more than {percent} percent.",
    "During the {place} migration the desk ran in parallel with the legacy ledger for {days} days; "
    "no differences remained when the legacy ledger was switched off.",
    "Operators must not edit entries in place; corrections are new entries that reverse and "
    "replace the original, so the audit trail stays complete.",
    "Large entries are flagged for a second review, but the flag does not change how the desk "
    "settles them.",
    "The desk publishes a short notice before any change to its rules takes effect, and the notice "
    "is copied into the history section of this page.",
    "Liquidity forecasts are prepared each morning from the previous day's released funds and the "
    "holds still running.",
    "The {place} office runs a fire drill every {days} weeks; during a drill the batch is paused "
    "and resumed from the last confirmed entry.",
    "New counterparties are onboarded by {owner}'s team, who check their sample files against the "
    "clearing format before the first live batch.",
    "Clearing statements are signed with the desk's key, which is rotated every {years} months by "
    "the security office in {place}.",
    "If the clearing window opens late, the desk keeps the entry dates the senders gave; nothing is "
    "re-dated to the opening day.",
    "The desk's dashboards show volumes per account and per day, refreshed every {days} minutes "
    "during the window.",
    "Two operators must approve any manual entry, and the second approver may not be from the "
    "same shift as the first.",
    "The annual audit samples {percent} entries at random and traces each one from the clearing "
    "statement to its settlement line.",
    "Settlement statements go out in the evening; counterparties in {place} receive them by the "
    "secure file drop, the others by the clearing network.",
    "A counterparty that sends a malformed file twice in a week is moved to the supervised queue "
    "until {owner} clears it.",
    "Fee income is booked to the desk's cost centre in {place} and reported monthly to finance.",
    "The desk's runbook lists {days} standard recoveries; anything outside them goes to the "
    "on-call engineer.",
    "Test batches are sent on the first business day of each month and must be labelled as tests "
    "in the sender's reference.",
)
COUNTERPARTY_A = ("Brack", "Corran", "Dunmore", "Elsby", "Farrow", "Gilder", "Harrow", "Ingle",
                  "Jessop", "Kerrin", "Larch", "Mossop", "Norrey", "Oakham", "Pryor", "Quill")
COUNTERPARTY_B = ("Securities", "Brokerage", "Custody", "Partners", "Clearing", "Capital",
                  "Trust", "Markets")
INCIDENTS = (
    "Clearing statement arrived {minutes} minutes late; batch confirmed after the delay.",
    "A counterparty resent its file with duplicate ids; the duplicates were returned to the sender.",
    "Dashboard refresh stalled for {minutes} minutes; no entries were affected.",
    "Manual entry approved by a single operator; reversed and re-entered with two approvals.",
    "Network maintenance in {place} moved the window by {minutes} minutes.",
    "A test batch was sent without its label; it was removed before settlement.",
    "Statement drop for {place} counterparties failed once and was retried successfully.",
    "Key rotation was postponed by a day at the security office's request.",
)
GLOSSARY = (
    ("Entry", "One booked movement of funds that the desk settles."),
    ("Line", "The settlement result for one accepted entry: amount, fee and net."),
    ("Floor", "The smallest amount the desk accepts."),
    ("Hold", "The period after the entry date before the desk releases funds."),
    ("Tier", "The settlement tier, used to group desks for operational changes."),
    ("Basis point", "One hundredth of a percent (1/10000)."),
    ("Release date", "The date on which held funds become available."),
    ("Correction run", "A later batch that replays amended entries."),
    ("Clearing window", "The period in which the desk accepts entries for the day."),
    ("Supervised queue", "Where files from counterparties under review wait for approval."),
)
# Each rule is written in one of several wordings, inside the section it belongs to, so the
# rules are read in context rather than picked out of one list.
FEE_TEXT = (
    "The desk charges {bps} basis points of each accepted amount; the result is {rounding}.",
    "Each accepted entry pays a fee of {bps} basis points of its amount, {rounding}.",
    "Fees are {bps} basis points of the amount, {rounding}.",
)
MIN_TEXT = (
    "No fee is lower than {min_fee} cents: a smaller computed fee is raised to it.",
    "The minimum fee is {min_fee} cents.",
    "A fee below {min_fee} cents is charged as {min_fee} cents.",
)
FLOOR_TEXT = (
    "Entries of less than {floor} cents are refused with error number {error}.",
    "The floor is {floor} cents; a smaller entry is rejected with error number {error}.",
    "Anything under {floor} cents is rejected, error number {error}.",
)
CURRENCY_TEXT = (
    "Only {currency} entries are settled; an entry in any other currency is rejected with error "
    "number {error}.",
    "The settlement currency is {currency}; entries in another currency get error number {error}.",
    "Entries not in {currency} are rejected with error number {error}.",
)
ORDER_LINE_TEXT = (
    "Settlement lines are listed {order}.",
    "The statement lists its lines {order}.",
    "Line order on the statement: {order}.",
)
HOLD_TEXT = (
    "Funds are held for {days} {calendar} after the entry date.",
    "The hold is {days} {calendar}, counted from the entry date.",
    "Released funds become available {days} {calendar} after the entry date.",
)


def _paragraph(rng, desk, templates, count):
    picks = rng.sample(templates, count)
    return " ".join(t.format(title=desk.title, region=desk.region, owner=desk.owner, place=rng.choice(PLACES),
                             days=rng.randrange(2, 9), years=rng.randrange(5, 12),
                             percent=rng.randrange(10, 40)) for t in picks)


def _section(rng, desk, *rules):
    """A section's prose with its rule sentences placed among filler sentences."""
    sentences = [rng.choice(FILLER) for _ in range(3)]
    sentences = [_paragraph(rng, desk, (s,), 1) for s in sentences]
    for rule in rules:
        sentences.insert(rng.randrange(0, len(sentences) + 1), rule)
    return " ".join(sentences)


# Bytes of each desk page; operations paragraphs are added until a page reaches it.
PAGE_BYTES = 14_000


def desk_page(desk, rng):
    old_bps = rng.choice([b for b in range(11, 98) if b != desk.bps])
    other_rounding = rng.choice([r for r in ROUNDINGS if r != desk.rounding])
    year = rng.randrange(2015, 2023)
    pick = rng.choice
    parts = [
        f"# {desk.title} desk", "",
        _paragraph(rng, desk, INTRO, 4), "",
        "## At a glance", "",
        "| Field | Value |", "| --- | --- |",
        f"| Desk code | `{desk.code}` |",
        f"| Module | `ledger/desks/{desk.name}.py` |",
        f"| Settlement tier | {desk.tier} |",
        f"| Clearing window | {desk.region} |",
        f"| Owner | {desk.owner} |",
        f"| Opened | {year - rng.randrange(3, 9)} |", "",
        "## Background", "",
        _paragraph(rng, desk, FILLER, 4), "",
        "## Accepting entries", "",
        _section(rng, desk, pick(FLOOR_TEXT).format(floor=desk.floor, error=desk.floor_error),
                 pick(CURRENCY_TEXT).format(currency=desk.currency, error=desk.currency_error)), "",
        "## Fees", "",
        _section(rng, desk, pick(FEE_TEXT).format(bps=desk.bps, rounding=ROUNDING_TEXT[desk.rounding]),
                 pick(MIN_TEXT).format(min_fee=desk.min_fee)), "",
        "## Statements", "",
        _section(rng, desk, pick(ORDER_LINE_TEXT).format(order=ORDER_TEXT[desk.order])), "",
        "## Funds availability", "",
        _section(rng, desk, pick(HOLD_TEXT).format(days=desk.hold_days, calendar=CALENDAR_TEXT[desk.calendar])),
        "",
    ]
    example = _entry(random.Random(rng.random()), desk, set(), desk.floor + rng.randrange(20_000, 90_000))
    line = engine.settle(desk.rules(), [example])["lines"][0]
    parts += ["## Worked example", "",
              f"An entry of {example['amount']} cents in {desk.currency} is accepted. The fee is "
              f"{line['fee']} cents and the net is {line['net']} cents.", "",
              "## Counterparties", "",
              "| Counterparty | Contact | Share of volume | Statement drop |", "| --- | --- | --- | --- |"]
    for _ in range(rng.randrange(8, 13)):
        parts.append(f"| {pick(COUNTERPARTY_A)} {pick(COUNTERPARTY_B)} | {pick(PEOPLE)} | "
                     f"{rng.randrange(1, 30)}% | {pick(PLACES)} |")
    parts += ["", "## Incident log", ""]
    for _ in range(rng.randrange(6, 10)):
        day = START - datetime.timedelta(days=rng.randrange(30, 900))
        parts.append(f"- {day.isoformat()}: " + pick(INCIDENTS).format(
            minutes=rng.randrange(5, 90), place=pick(PLACES)))
    parts += ["", "## History", "",
              f"- Until {year} the desk charged {old_bps} basis points. That rate is retired.",
              f"- A {year + 1} proposal to round fees {other_rounding.replace('_', ' ')} was rejected.",
              f"- In {year + 2} the floor was reviewed and kept; the review notes are filed in {desk.region}.",
              "", "## Glossary", ""]
    parts += [f"- **{term}**: {meaning}" for term, meaning in GLOSSARY]
    parts += ["", "## Operations", ""]
    while len("\n".join(parts).encode()) < PAGE_BYTES:
        parts += [_paragraph(rng, desk, FILLER, 4), ""]
    return "\n".join(parts)


DESK_SOURCE = '''"""{title} desk: settlement for entries booked through the {region} clearing window.

The desk's rules are on its page, docs/desks/{name}.md; the shared rules are in
docs/overview.md.
"""

from ledger.common import check_entry, group_by_account, money

NAME = "{name}"
TITLE = "{title} desk"


def describe():
    """A one-line description for operators' tooling."""
    return f"{{TITLE}} ({{NAME}})"


def account_totals(entries):
    """Total amount per account, in cents, for the operations dashboard."""
    totals = {{}}
    for account, rows in group_by_account(entries).items():
        totals[account] = sum(row["amount"] for row in rows)
    return totals


def largest_entry(entries):
    """The entry with the largest amount (the first one on a tie), or None."""
    best = None
    for entry in entries:
        check_entry(entry)
        if best is None or entry["amount"] > best["amount"]:
            best = entry
    return best


def format_totals(entries):
    """Lines like `AC-BKD 12.50` for the {region} statement, sorted by account."""
    return [f"{{account}} {{money(total)}}" for account, total in sorted(account_totals(entries).items())]


def {helper}(entries):
    """Entries booked on each date, as a dict from date to count ({owner}'s morning forecast)."""
    counts = {{}}
    for entry in entries:
        counts[entry["date"]] = counts.get(entry["date"], 0) + 1
    return counts


def settle(entries):
    """Settles entries as docs/overview.md and docs/desks/{name}.md describe."""
    raise NotImplementedError("settlement for the {title} desk is not written yet")
'''

COMMON = '''"""Helpers shared by the desk modules."""

import csv

FIELDS = ("id", "account", "amount", "currency", "date")


def check_entry(entry):
    """Raises ValueError when an entry lacks a field or has a non-integer amount."""
    missing = [field for field in FIELDS if field not in entry]
    if missing:
        raise ValueError(f"entry is missing {{', '.join(missing)}}")
    if not isinstance(entry["amount"], int):
        raise ValueError("amount must be an int number of cents")


def group_by_account(entries):
    """A dict from account to its entries, in the order given."""
    groups = {{}}
    for entry in entries:
        groups.setdefault(entry["account"], []).append(entry)
    return groups


def money(cents):
    """Cents as a decimal string: 1234 -> '12.34'."""
    return f"{{cents // 100}}.{{cents % 100:02d}}"


def read_entries(path):
    """Entries from a CSV file with the header id,account,amount,currency,date."""
    with open(path, newline="", encoding="utf-8") as handle:
        return [dict(row, amount=int(row["amount"])) for row in csv.DictReader(handle)]
'''

README = """# Ledger settlement service

A settlement service for {count} trading desks. Read `docs/overview.md` first: it explains entries,
settlement and holds. Each desk has a module in `ledger/desks/` and a page in `docs/desks/` with its
own rules. Sample entries are in `data/`.

Standard library only, Python 3.10 or later. Tests live in `tests/` and run with
`python3 -m unittest discover -s tests`.
"""


def _write(root, relative, text):
    path = os.path.join(root, relative)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def data_rows(desk, rng, target_bytes):
    """CSV text of sample entries, about `target_bytes` long."""
    ids, lines = set(), ["id,account,amount,currency,date"]
    size = len(lines[0]) + 1
    while size < target_bytes:
        roll = rng.random()
        amount = rng.randrange(desk.floor, desk.floor + 500_000) if roll > 0.1 else rng.randrange(1, desk.floor)
        entry = _entry(rng, desk, ids, amount, currency=_foreign(desk) if roll > 0.95 else None)
        line = ",".join(str(entry[k]) for k in ("id", "account", "amount", "currency", "date"))
        lines.append(line)
        size += len(line) + 1
    return "\n".join(lines) + "\n"


# Bytes of material per desk beyond its page and module, filled with sample data; tuned so each
# size lands near its target.
PER_DESK_BYTES = 19_200


def generate(size, seed, project):
    """Writes the project for (size, seed) into `project` and returns its Spec."""
    spec = make_spec(size, seed)
    rng = random.Random(f"budget-project-{size}-{seed}")
    count = len(spec.desks)
    _write(project, "README.md", README.format(count=count))
    _write(project, "docs/overview.md", OVERVIEW.format(count=count))
    _write(project, "ledger/__init__.py", '"""Ledger settlement service."""\n')
    _write(project, "ledger/common.py", COMMON.format())
    _write(project, "ledger/desks/__init__.py", '"""The trading desks, one module each."""\n\nDESKS = [\n'
           + "".join(f'    "{d.name}",\n' for d in spec.desks) + "]\n")
    os.makedirs(os.path.join(project, "tests"), exist_ok=True)
    for desk in spec.desks:
        page = desk_page(desk, rng)
        helper = rng.choice(("entries_per_day", "daily_counts", "booking_calendar", "dates_seen"))
        source = DESK_SOURCE.format(title=desk.title, region=desk.region, name=desk.name, owner=desk.owner,
                                    helper=helper)
        _write(project, f"docs/desks/{desk.name}.md", page)
        _write(project, f"ledger/desks/{desk.name}.py", source)
        _write(project, f"data/{desk.name}.csv",
               data_rows(desk, rng, max(1000, PER_DESK_BYTES - len(page) - len(source))))
    return spec


SKIP = {".git", ".themis", "__pycache__", "tests"}


def material_tokens(project):
    """Tokens (bytes / 4) of everything the agent receives, tests excluded."""
    total = 0
    for folder, dirs, files in os.walk(project):
        dirs[:] = [d for d in dirs if d not in SKIP]
        total += sum(os.path.getsize(os.path.join(folder, f)) for f in files)
    return total // 4


def in_band(size, tokens):
    target = TARGET_TOKENS[size]
    return target * (1 - BAND) <= tokens <= target * (1 + BAND)


# ---------------------------------------------------------------------------------------------
# Reference solution (never shipped to the agent)

REFERENCE_DESK = '''"""{title} desk (reference solution)."""

from ledger import engine

RULES = {rules}


def settle(entries):
    return engine.settle(RULES, entries)


def holds(entries, today):
    return engine.holds(RULES, entries, today)
'''

REFERENCE_TEST = '''import unittest

from ledger.desks import DESKS


class ReferenceTests(unittest.TestCase):
    def test_every_desk_settles(self):
        import importlib
        for name in DESKS:
            self.assertIn("lines", importlib.import_module("ledger.desks." + name).settle([]))
'''

HERE = os.path.dirname(os.path.abspath(__file__))


def write_reference(spec, project):
    """Overlays the reference solution of every stage onto a generated project."""
    with open(os.path.join(HERE, "reference", "engine.py"), encoding="utf-8") as handle:
        _write(project, "ledger/engine.py", handle.read())
    with open(os.path.join(HERE, "reference", "report.py"), encoding="utf-8") as handle:
        _write(project, "ledger/__main__.py", handle.read())
    for desk in spec.desks:
        _write(project, f"ledger/desks/{desk.name}.py",
               REFERENCE_DESK.format(title=desk.title, rules=repr(spec.final_rules(desk))))
    _write(project, "tests/test_reference.py", REFERENCE_TEST)
