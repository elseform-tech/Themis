"""Regenerates the synthetic fixtures under `cases/` deterministically.

    python3 -m evals.compaction.gen_cases

Every fact a probe asks about is written by hand below; only the surrounding
file contents, logs and prose are generated filler. Fixtures contain no personal
data. Run this after editing a scenario and commit the regenerated files.
"""

import os

from .fixturekit import Transcript, finish, ident, python_module, rust_module

FUNCS = 9
CASES = os.path.join(os.path.dirname(__file__), "cases")


def P(pid, category, question, planted_at, any_of=None, regex=None, stale=None, expect_unknown=False,
      needle=None):
    probe = {"id": pid, "category": category, "question": question, "planted_at": planted_at}
    if needle:
        probe["needle"] = needle  # must appear in the planted record (checked by finish)
    if expect_unknown:
        probe["expect_unknown"] = True
    else:
        probe["expected"] = {k: v for k, v in (("any_of", any_of), ("regex", regex)) if v}
    if stale:
        probe["stale"] = stale
    return probe


def explore(t, count, lang, folder, note):
    """`count` filler read_file steps over generated modules."""
    for i in range(count):
        name = ident(t.rng)
        ext = "rs" if lang == "rust" else "py"
        path = f"{folder}/{name}.{ext}"
        body = rust_module(t.rng, FUNCS) if lang == "rust" else python_module(t.rng, FUNCS)
        t.call("read_file", {"file_path": path}, body, text=f"{note} Checking {path}." if i == 0 else "")


def shell(t, command, output, text=""):
    return t.call("shell", {"command": command}, output, text=text)


# --------------------------------------------------------------------- coding

def coding_rate_limiter():
    t = Transcript(11)
    t.user("We're adding per-API-key rate limiting to the pinboard-api service. Constraints: use the token-bucket algorithm (not sliding window), and do not add any new third-party dependency. Start by looking at how requests flow through middleware.")
    c_algo = 1
    t.asst("Understood: token bucket, standard library only. I'll read the middleware first.")
    explore(t, 4, "python", "pinboard/middleware", "Mapping the request path.")
    c_settings = shell(t, "cat pinboard/settings.py", "RATE_LIMIT_BURST = 25\nREQUEST_TIMEOUT_S = 12\nAPI_KEY_HEADER = 'X-Api-Key'\n")
    t.asst("Settings already expose RATE_LIMIT_BURST=25. I'll create the limiter package with a Clock seam.")
    t.call("write_file", {"file_path": "pinboard/limiter/bucket.py", "content": "class TokenBucket: ..."}, '{"ok":true,"path":"pinboard/limiter/bucket.py"}')
    explore(t, 4, "python", "pinboard/handlers", "Looking at handlers that need a limit.")
    t.user("Tiers: free tier gets 60 requests per minute, paid tier gets 600 per minute.")
    c_tier = len(t.records)
    t.asst("Got it: free 60/min, paid 600/min. I'll wire those into the limiter config.")
    t.call("write_file", {"file_path": "pinboard/limiter/config.py", "content": "TIERS = {'free': 60, 'paid': 600}"}, '{"ok":true,"path":"pinboard/limiter/config.py"}')
    explore(t, 3, "python", "pinboard/storage", "Reviewing storage helpers.")
    t.user("For the response header, use X-Rate-Remaining to show tokens left.")
    t.asst("Using X-Rate-Remaining for the remaining-token header.")
    t.boundary()
    explore(t, 6, "python", "pinboard/utils", "Checking utility modules before the first test run.")
    shell(t, "pytest tests/test_limits.py -x", "============ FAILURES ============\n_ test_burst_exceeds _\nAssertionError: expected 429 got 200\ntests/test_limits.py:41\n1 failed, 13 passed in 2.31s")
    t.asst("test_burst_exceeds fails. Let me trace the refill computation.")
    c_bucket = t.call("read_file", {"file_path": "pinboard/limiter/bucket.py"}, "import time\nclass TokenBucket:\n    def refill(self):\n        now = time.time()\n        self.tokens = min(self.cap, self.tokens + (now - self.last) * self.rate)\n        self.last = now\n")
    c_root = c_bucket
    t.asst("Root cause: refill() uses time.time(), which can jump when the wall clock is adjusted, so the test clock and the bucket disagree. I'll switch to time.monotonic and inject the clock from pinboard/limiter/clock.py.")
    c_clockfile = t.call("write_file", {"file_path": "pinboard/limiter/clock.py", "content": "import time\nclass Clock:\n    def now(self): return time.monotonic()"}, '{"ok":true,"path":"pinboard/limiter/clock.py"}')
    shell(t, "pytest tests/test_limits.py", "14 passed, 1 skipped in 2.02s")
    c_pass = len(t.records)
    t.asst("All limiter tests pass (14 passed, 1 skipped: the redis backend test, since we have no redis configured).")
    explore(t, 5, "python", "pinboard/views", "Scanning views for the header change.")
    t.user("Correction on the header: it should be X-RateLimit-Remaining, not X-Rate-Remaining. Please rename it everywhere.")
    c_hdr2 = len(t.records)
    t.call("shell", {"command": "grep -rl X-Rate-Remaining pinboard tests | xargs sed -i 's/X-Rate-Remaining/X-RateLimit-Remaining/g'"}, "renamed in 3 files")
    t.asst("Renamed to X-RateLimit-Remaining in 3 files.")
    t.boundary()
    explore(t, 4, "python", "pinboard/jobs", "Final sweep.")
    shell(t, "pytest -q", "27 passed, 1 skipped in 4.88s")
    t.asst("Full suite is green. Remaining follow-ups: update docs/limits.md with the tier table and open a follow-up for per-IP limits for unauthenticated routes.")
    probes = [
        P("algorithm", "constraint", "Which rate-limiting algorithm was the user required to use?", c_algo, any_of=["token bucket", "token-bucket"], stale=["sliding window"]),
        P("deps", "constraint", "Is adding a new third-party dependency allowed for this feature? Answer yes or no.", c_algo, regex=r"^\W*no\b", needle=r"new third-party dependency"),
        P("free_tier", "identifier", "How many requests per minute does the free tier get?", c_tier, regex=r"\b60\b(?!\d)"),
        P("paid_tier", "identifier", "How many requests per minute does the paid tier get?", c_tier, regex=r"\b600\b"),
        P("root_cause", "tool_result", "What was the root cause of the failing test_burst_exceeds?", c_root, any_of=["time.time", "wall clock", "wall-clock", "monotonic"]),
        P("header", "superseded", "What is the final name of the response header showing remaining tokens?", c_hdr2, regex=r"X-RateLimit-Remaining", stale=[r"X-Rate-Remaining"]),
        P("clock_file", "identifier", "Which file holds the injected clock?", c_clockfile, any_of=["limiter/clock.py"]),
        P("burst", "tool_result", "What is RATE_LIMIT_BURST set to in settings.py?", c_settings, regex=r"\b25\b"),
        P("skipped", "tool_result", "Why was one test skipped?", c_pass + 1, any_of=["redis"]),
        P("redis_host", "decoy", "What Redis hostname is the limiter configured to use?", 1, expect_unknown=True),
        P("next", "next_action", "What are the remaining follow-up tasks?", len(t.records), regex=r"(?is)(docs/limits|tier table|documentation).*(per-IP|ip)|(per-IP|ip).*(docs/limits|tier table|documentation)"),
    ]
    finish(os.path.join(CASES, "coding-rate-limiter"), "coding-rate-limiter", "coding", t, probes,
           "Update docs/limits.md with the tier table; open a follow-up for per-IP limits.",
           "Python service: token-bucket limiter with a mid-session header rename and a clock-related test failure.")


def coding_csv_migration():
    t = Transcript(22)
    t.user("I need a script that imports the legacy order export into SQLite. The export is semicolon-delimited, encoded in windows-1252, and amounts use a comma decimal separator like 12,50. Never modify data/legacy/orders_2019.csv in place; always work on a copy. If any row is bad, abort the whole import.")
    c_fmt = 1
    t.asst("Plan: read with csv.reader(delimiter=';', encoding='cp1252'), convert comma decimals, and operate only on a copy. I'll abort on the first bad row as you asked.")
    explore(t, 5, "python", "ordersync/legacy", "Surveying the legacy loaders.")
    shell(t, "head -3 data/legacy/orders_2019.csv", "order_id;customer_id;sku;qty;amount\n1001;C-204;AX-17;2;12,50\n1002;C-311;BQ-02;1;7,90\n")
    t.asst("Confirmed the format. Writing importer.")
    t.call("write_file", {"file_path": "ordersync/importer.py", "content": "def import_orders(path): ..."}, '{"ok":true,"path":"ordersync/importer.py"}')
    explore(t, 5, "python", "ordersync/db", "Reviewing db helpers.")
    t.user("Actually, change of plan on bad rows: do not abort. Skip rows with an empty customer_id and write them to rejects.csv instead.")
    c_rej = len(t.records)
    t.asst("Switching to skip-and-record: bad rows with empty customer_id go to rejects.csv, import continues.")
    t.boundary()
    explore(t, 6, "python", "ordersync/validators", "Reading validators.")
    shell(t, "pytest tests/test_importer.py", "FAILED tests/test_importer.py::test_full_file - ValueError: invalid literal for int() with base 10: '3 '\n  at row 8812\n1 failed, 9 passed")
    c_err = len(t.records)
    t.asst("Failure at row 8812: the qty column has trailing whitespace ('3 '). Fix: strip qty before int().")
    t.call("write_file", {"file_path": "ordersync/importer.py", "content": "qty = int(row['qty'].strip())"}, '{"ok":true,"path":"ordersync/importer.py"}')
    shell(t, "pytest tests/test_importer.py", "10 passed in 3.40s")
    explore(t, 6, "python", "ordersync/reports", "Looking at report modules.")
    shell(t, "python -m ordersync.importer --copy data/work/orders_copy.csv", "imported 41207 rows, rejected 93 rows -> rejects.csv")
    c_final = len(t.records)
    t.asst("Import finished: 41207 rows imported, 93 rejected. Next I'd add an index on customer_id and document the rejects.csv format in the README.")
    probes = [
        P("delimiter", "identifier", "What delimiter does the legacy export use?", c_fmt, any_of=["semicolon", ";"]),
        P("encoding", "identifier", "What character encoding does the legacy export use?", c_fmt, regex=r"(?i)windows-?1252|cp-?1252"),
        P("inplace", "constraint", "Which file must never be modified in place?", c_fmt, any_of=["orders_2019.csv"]),
        P("bad_rows", "superseded", "What should the importer do with rows that have an empty customer_id?", c_rej, regex=r"(?i)reject|skip", stale=[r"(?i)abort(s|ed)?\b(?!.*instead)"]),
        P("bad_row_no", "tool_result", "Which row number caused the ValueError in test_full_file?", c_err, regex=r"\b8812\b"),
        P("cause", "tool_result", "What was the cause of that ValueError?", c_err + 1, regex=r"(?i)\bwhitespace\b|\bspaces?\b|\bstrip"),
        P("rows", "tool_result", "How many rows were imported and how many rejected in the final run?", c_final, regex=r"(?s)41,?207.*\b93\b"),
        P("decimal", "identifier", "What decimal separator do amounts use in the export?", c_fmt, regex=r"(?i)\bcomma\b"),
        P("page_size", "decoy", "What SQLite page_size did we configure?", 1, expect_unknown=True),
        P("next", "next_action", "What is the proposed next step?", len(t.records), regex=r"(?is)index.*customer_id|readme"),
    ]
    finish(os.path.join(CASES, "coding-csv-migration"), "coding-csv-migration", "coding", t, probes,
           "Add an index on customer_id and document the rejects.csv format in the README.",
           "Legacy CSV import with a mid-session policy reversal and a data-quality failure.")


def coding_rust_parser():
    t = Transcript(33)
    t.user("The semver-range crate minisv has a bug: caret ranges like ^0.0.3 match 0.0.4. Fix it. Constraints: MSRV is 1.70 so no let-else, no unsafe code, no new dependencies, and clippy must pass with -D warnings. Return Err(ParseError::EmptyComparator) for empty comparators instead of panicking.")
    c_con = 1
    t.asst("Noted: MSRV 1.70, no unsafe, no new deps, clippy -D warnings, and EmptyComparator instead of panics. Starting with the test run.")
    shell(t, "cargo test", "running 33 tests\n...\ntest range::caret_zero_zero_patch ... FAILED\ntest range::tilde_missing_minor ... FAILED\ntest result: FAILED. 31 passed; 2 failed")
    c_fail = len(t.records)
    t.asst("Two failures: caret_zero_zero_patch and tilde_missing_minor.")
    explore(t, 8, "rust", "src/range", "Reading the range module.")
    t.asst("I could rewrite the parser with the regex crate, which would make this simpler.")
    t.user("No, do not pull in regex or any other crate. Keep the hand-written parser.")
    c_noreg = len(t.records)
    t.asst("Agreed, keeping the hand-written parser with no new crates.")
    t.boundary()
    explore(t, 6, "rust", "src/version", "Reading version helpers.")
    c_bug = t.call("read_file", {"file_path": "src/range/caret.rs"}, "pub fn upper_bound(v: &Version) -> Version {\n    let idx = first_nonzero(v);\n    bump_index(v, idx)\n}\nfn bump_index(v: &Version, idx: usize) -> Version {\n    // BUG: should be idx < 2, was <=\n    if idx <= 2 { v.bump(idx) } else { v.clone() }\n}\n")
    t.asst("Found it: bump_index in src/range/caret.rs compares idx <= 2 where it must be idx < 2 for the patch position, an off-by-one.")
    t.call("write_file", {"file_path": "src/range/caret.rs", "content": "if idx < 2 { v.bump(idx) } else { v.next_patch() }"}, '{"ok":true,"path":"src/range/caret.rs"}')
    explore(t, 6, "rust", "src/parse", "Checking the comparator parsing path.")
    t.call("write_file", {"file_path": "src/parse/comparator.rs", "content": "if text.is_empty() { return Err(ParseError::EmptyComparator); }"}, '{"ok":true,"path":"src/parse/comparator.rs"}')
    shell(t, "cargo test", "running 34 tests\ntest result: ok. 34 passed; 0 failed")
    c_pass = len(t.records)
    t.asst("All 34 tests pass (33 existing plus the new empty-comparator test).")
    shell(t, "cargo clippy --all-targets -- -D warnings", "warning: unused variable: `idx`\n  --> src/range/tilde.rs:19\nerror: could not compile due to previous error")
    c_clip = len(t.records)
    t.asst("Clippy flags an unused variable idx at src/range/tilde.rs:19. Fixing by renaming to _idx.")
    t.call("write_file", {"file_path": "src/range/tilde.rs", "content": "let _idx = 0;"}, '{"ok":true}')
    shell(t, "cargo clippy --all-targets -- -D warnings", "Finished dev profile, no warnings")
    t.asst("Clippy clean. Remaining: add a CHANGELOG entry and bump the version to 0.4.2.")
    probes = [
        P("msrv", "constraint", "What is the minimum supported Rust version, and which syntax is therefore banned?", c_con, regex=r"(?is)1\.70.*let-?else|let-?else.*1\.70"),
        P("no_regex", "negation", "Is it acceptable to add the regex crate to simplify the parser?", c_noreg, regex=r"(?i)^\W*no\b|\bnot (acceptable|allowed|permitted|be added)\b|\bdo not\b|\bnever\b"),
        P("fail_tests", "tool_result", "Which two tests failed initially?", c_fail, regex=r"(?is)caret_zero_zero_patch.*tilde_missing_minor|tilde_missing_minor.*caret_zero_zero_patch"),
        P("bug_file", "identifier", "In which file and function was the off-by-one found?", c_bug + 1, regex=r"(?is)caret\.rs.*bump_index|bump_index.*caret\.rs"),
        P("bug_cmp", "tool_result", "What was the incorrect comparison, and what replaced it?", c_bug, regex=r"<=.*<|<\s*2"),
        P("empty", "constraint", "What should an empty comparator return?", c_con, any_of=["EmptyComparator"], stale=[r"(?i)panic"]),
        P("final_tests", "tool_result", "How many tests passed in the final cargo test run?", c_pass, regex=r"\b34\b"),
        P("clippy", "tool_result", "Where did clippy complain and about what?", c_clip, regex=r"(?is)tilde\.rs.*(19|unused)|unused.*tilde"),
        P("rustc_version", "decoy", "Which exact rustc version is installed on the build machine?", 1, expect_unknown=True),
        P("next", "next_action", "What remains to be done?", len(t.records), regex=r"(?is)changelog.*0\.4\.2|0\.4\.2.*changelog"),
    ]
    finish(os.path.join(CASES, "coding-rust-parser"), "coding-rust-parser", "coding", t, probes,
           "Add a CHANGELOG entry and bump the version to 0.4.2.",
           "Rust crate bug fix with negated suggestion (no regex) and constraint set.")


# ------------------------------------------------------------- topic switching

def parked_trip_bugfix():
    t = Transcript(44)
    t.scale = 4
    t.user("Let's plan a trip to Lisbon. We travel March 14 to March 19, budget is 1800 EUR total, I'm allergic to peanuts, and I love seafood. We want a hotel in Alfama.")
    c_trip = 1
    t.asst("Lisbon, March 14-19, 1800 EUR, peanut allergy, seafood lover, Alfama stay. " + t.prose(10))
    for i in range(6):
        t.user(f"Day {i + 1}: what should we do?")
        t.asst(f"Day {i + 1} idea. " + t.prose(14))
    t.user("For the hotel, I like Hotel Miradouro Verde at 96 EUR per night. Let's pencil that in.")
    c_hotel = len(t.records)
    t.asst("Penciled in Hotel Miradouro Verde, 96 EUR/night, 5 nights = 480 EUR. " + t.prose(8))
    t.user("Park the trip planning for now. I need to fix a bug in my invoice script first.")
    c_park = len(t.records)
    t.asst("Parked: Lisbon trip (Mar 14-19, 1800 EUR, Hotel Miradouro Verde, peanut allergy). Switching to the invoice script.")
    t.boundary()
    explore(t, 6, "python", "invoicer", "Reading the invoice code.")
    shell(t, "pytest tests/test_totals.py", "FAILED tests/test_totals.py::test_vat_rounding - assert Decimal('10.05') == Decimal('10.04')\n1 failed, 7 passed")
    c_bug = len(t.records)
    t.asst("test_vat_rounding fails by one cent. The function round_cents in invoicer/totals.py rounds half up; the spec expects banker's rounding.")
    t.call("write_file", {"file_path": "invoicer/totals.py", "content": "return amount.quantize(Decimal('0.01'), rounding=ROUND_HALF_EVEN)"}, '{"ok":true,"path":"invoicer/totals.py"}')
    c_fix = len(t.records)
    explore(t, 6, "python", "invoicer/export", "Checking exporters for the same rounding.")
    shell(t, "pytest tests/test_totals.py", "8 passed in 0.9s")
    t.asst("Fixed: round_cents now uses ROUND_HALF_EVEN. Tests pass (8 passed).")
    t.boundary()
    explore(t, 3, "python", "invoicer/misc", "Final sweep.")
    t.user("Great, the bug is done. Back to the trip: remind me what's pending there.")
    t.asst("Pending for Lisbon: book the flights and reserve a seafood dinner in Alfama; remember the peanut allergy when choosing restaurants.")
    probes = [
        P("dates", "parked_topic", "What dates is the Lisbon trip?", c_trip, regex=r"(?i)March 14.*(March )?19|14.*19 March|Mar(ch)? 14"),
        P("budget", "parked_topic", "What is the total trip budget?", c_trip, regex=r"1,?800"),
        P("allergy", "parked_topic", "What food allergy must be remembered for the trip?", c_trip, any_of=["peanut"]),
        P("hotel", "parked_topic", "Which hotel was penciled in and at what nightly price?", c_hotel, regex=r"(?is)Miradouro Verde.*96|96.*Miradouro Verde"),
        P("neighborhood", "parked_topic", "Which neighborhood did the user want to stay in?", c_trip, any_of=["Alfama"]),
        P("bug_func", "identifier", "Which function had the rounding bug and in which file?", c_bug + 1, regex=r"(?is)round_cents.*totals\.py|totals\.py.*round_cents"),
        P("rounding", "decision", "Which rounding mode does the fix use?", c_fix - 1, regex=r"(?i)HALF_EVEN|banker"),
        P("park_state", "parked_topic", "What was the status of the trip planning while the bug was fixed?", c_park, regex=r"(?i)park|paus|on hold"),
        P("flight", "decoy", "Which airline and flight number was chosen for the trip?", 1, expect_unknown=True),
        P("next", "next_action", "What is pending for the trip?", len(t.records), regex=r"(?is)flight.*(dinner|restaurant)|(dinner|restaurant).*flight"),
    ]
    finish(os.path.join(CASES, "parked-trip-bugfix"), "parked-trip-bugfix", "parked_topic", t, probes,
           "Return to the Lisbon trip: book flights and reserve a seafood dinner.",
           "Trip planning parked while a coding bug is fixed; both threads probed afterwards.")


def parked_budget_recipe():
    t = Transcript(55)
    t.scale = 2
    t.user("Help me build a monthly budget script. Categories: rent 1450, groceries 420, transit 95, and a savings goal of 15 percent of income. Income is 4200 per month.")
    c_budget = 1
    t.asst("Budget skeleton created with rent 1450, groceries 420, transit 95, savings goal 15% of 4200 (630).")
    explore(t, 6, "python", "budget", "Reading existing budget modules.")
    t.call("write_file", {"file_path": "budget/plan.py", "content": "RENT=1450; GROCERIES=420; TRANSIT=95; SAVINGS_PCT=0.15"}, '{"ok":true,"path":"budget/plan.py"}')
    t.user("Unrelated: what's a good buttermilk substitute for pancakes? Park the budget for a minute. I'm lactose intolerant.")
    c_recipe = len(t.records)
    t.asst("Budget parked. For lactose-free pancakes: use 1 cup oat milk plus 1 tablespoon lemon juice per cup of buttermilk, rest 5 minutes. " + t.prose(10))
    for i in range(4):
        t.user(f"Pancake follow-up {i + 1}: any tweaks?")
        t.asst(f"Tweak {i + 1}. " + t.prose(14))
    t.user("Final recipe: 2 cups flour, 1 egg, the oat-milk mixture. Cook at medium-low. Thanks! Park that too.")
    c_final_recipe = len(t.records)
    t.asst("Recipe parked: 2 cups flour, 1 egg, oat milk and lemon juice, medium-low heat.")
    t.boundary()
    t.user("New topic: gift ideas for my sister's 30th birthday on November 9. Budget is 80 dollars. She likes ceramics and hiking.")
    c_gift = len(t.records)
    t.asst("Gift ideas under 80 dollars for ceramics and hiking fans. " + t.prose(16))
    for i in range(5):
        t.user(f"Gift option {i + 1}: tell me more.")
        t.asst(f"Option {i + 1}. " + t.prose(16))
    t.user("I'll go with a hand-thrown mug set. Park the gift thing, and let's go back to the budget.")
    c_mug = len(t.records)
    t.asst("Gift decided: hand-thrown mug set. Returning to the budget.")
    explore(t, 6, "python", "budget/reports", "Looking at report code.")
    shell(t, "pytest tests/test_plan.py", "FAILED tests/test_plan.py::test_savings_goal - assert 630 == 640\n1 failed, 4 passed")
    c_fail = len(t.records)
    t.asst("test_savings_goal expects 640 but 15% of 4200 is 630; the test constant was wrong. Updating it to 630.")
    t.call("write_file", {"file_path": "tests/test_plan.py", "content": "assert savings == 630"}, '{"ok":true}')
    shell(t, "pytest tests/test_plan.py", "5 passed")
    t.asst("Budget tests pass. Next I'll add a CSV export for the monthly plan.")
    probes = [
        P("rent", "parked_topic", "What is the monthly rent in the budget?", c_budget, regex=r"1,?450"),
        P("savings", "parked_topic", "What savings target is set, as a percentage and amount?", c_budget + 1, regex=r"(?is)15.*630|630.*15"),
        P("income", "parked_topic", "What is the monthly income?", c_budget, regex=r"4,?200"),
        P("diet", "parked_topic", "What dietary restriction affects the recipe?", c_recipe, any_of=["lactose"]),
        P("substitute", "parked_topic", "What buttermilk substitute was chosen?", c_recipe + 1, regex=r"(?is)oat milk.*lemon|lemon.*oat"),
        P("flour", "parked_topic", "How much flour does the final recipe use?", c_final_recipe, regex=r"\b2 cups\b|two cups"),
        P("gift", "parked_topic", "Which gift was chosen for the sister?", c_mug, regex=r"(?i)mug"),
        P("gift_date", "parked_topic", "When is the sister's birthday?", c_gift, regex=r"(?i)Nov(ember)?\.? 9|9 Nov"),
        P("gift_budget", "parked_topic", "What is the gift budget?", c_gift, regex=r"\b80\b"),
        P("test_fail", "tool_result", "Why did test_savings_goal fail?", c_fail + 1, regex=r"(?is)640.*630|630.*640|test (constant|expectation)|expected (value|constant)"),
        P("oven", "decoy", "What oven temperature does the pancake recipe need?", 1, expect_unknown=True),
        P("next", "next_action", "What is the next step on the budget?", len(t.records), regex=r"(?i)csv"),
    ]
    finish(os.path.join(CASES, "parked-budget-recipe"), "parked-budget-recipe", "parked_topic", t, probes,
           "Add a CSV export for the monthly budget plan.",
           "Budget script interrupted by two parked side topics (recipe, gift) then resumed.")


# ------------------------------------------------- superseded / negation / constraints

def superseded_deploy_plan():
    t = Transcript(66)
    t.user("We're planning the production deployment of the checkout service. First draft: region us-east-1, Postgres 15, instance type t3.medium, cutover on November 4. Do not use Redis for caching.")
    c_draft = 1
    t.asst("Draft recorded: us-east-1, Postgres 15, t3.medium, cutover Nov 4, no Redis. " + t.prose(8))
    explore(t, 6, "python", "infra/modules", "Reading the Terraform helper scripts.")
    t.user("Legal says customer data must stay in the EU. Change the region to eu-west-1.")
    c_reg1 = len(t.records)
    t.asst("Region changed to eu-west-1. " + t.prose(8))
    explore(t, 5, "python", "infra/network", "Checking network modules.")
    t.user("Also we are NOT migrating the analytics database this quarter. Leave it alone.")
    c_analytics = len(t.records)
    t.asst("Understood: analytics database is out of scope this quarter.")
    t.user("Load test shows t3.medium is too small. Use t3.large.")
    c_size = len(t.records)
    t.asst("Instance type updated to t3.large.")
    t.boundary()
    explore(t, 4, "python", "infra/db", "Reviewing database provisioning.")
    shell(t, "psql -c 'select version()'", "PostgreSQL 16.2 on x86_64-pc-linux-gnu")
    t.user("The managed service only offers Postgres 16 now. Use Postgres 16, not 15.")
    c_pg = len(t.records)
    t.asst("Updated to Postgres 16.")
    t.user("Most customers are in Frankfurt; latency matters more than I thought. Switch the region again to eu-central-1.")
    c_reg2 = len(t.records)
    t.asst("Region is now eu-central-1 (final). " + t.prose(6))
    explore(t, 3, "python", "infra/monitoring", "Checking alert config.")
    t.user("Cutover slips a week: November 11 instead of November 4.")
    c_date = len(t.records)
    t.asst("Cutover moved to November 11.")
    explore(t, 3, "python", "infra/runbooks", "Last review of runbooks.")
    t.asst("Summary of current plan ready. Next step is to write the rollback runbook and schedule the load test rerun on t3.large.")
    probes = [
        P("region", "superseded", "What is the final deployment region?", c_reg2, regex=r"eu-central-1", stale=[r"us-east-1", r"eu-west-1"]),
        P("pg", "superseded", "Which Postgres version will be used?", c_pg, regex=r"\b16\b", stale=[r"(?<![\d.])15(?![\d])"]),
        P("size", "superseded", "What instance type will be used?", c_size, regex=r"t3\.large", stale=[r"t3\.medium"]),
        P("date", "superseded", "When is the cutover?", c_date, regex=r"(?i)Nov(ember)?\.? 11|11 Nov", stale=[r"(?i)Nov(ember)?\.? 4\b|\b4 Nov"]),
        P("redis", "negation", "Is Redis to be used for caching?", c_draft, regex=r"(?i)^\W*no\b|\bnot\b.{0,20}\buse[d]?\b|\bdo not use\b|\bnever\b"),
        P("analytics", "negation", "Will the analytics database be migrated this quarter?", c_analytics, regex=r"(?i)^\W*no\b|\bnot\b.{0,25}\bmigrat\w*|\bout of scope\b"),
        P("why_eu", "decision", "Why was the region moved to the EU first?", c_reg1, regex=r"(?i)\blegal\b|\bdata\b.{0,40}\b(stay|resid\w*|remain)\b"),
        P("why_fra", "decision", "Why was eu-central-1 chosen over eu-west-1?", c_reg2, regex=r"(?i)Frankfurt|latency"),
        P("backup", "decoy", "What backup retention period did we agree on?", 1, expect_unknown=True),
        P("next", "next_action", "What is the next step?", len(t.records), regex=r"(?is)rollback.*load test|load test.*rollback"),
    ]
    finish(os.path.join(CASES, "superseded-deploy-plan"), "superseded-deploy-plan", "superseded", t, probes,
           "Write the rollback runbook and rerun the load test on t3.large.",
           "Deployment plan where region, DB version, size and date each change; plus negations.")


def constraints_api_design():
    t = Transcript(77)
    t.user("We're extending the billing API. Rules: never change the response schema of /v1/users, add no new dependencies, and keep replies terse, bullet style. Pagination should use offset and limit.")
    c_rules = 1
    t.asst("- No /v1/users schema changes\n- No new deps\n- Offset/limit pagination\n- Terse replies")
    explore(t, 6, "python", "billing/api", "Reading API modules.")
    t.user("Rename the field fullName to display_name in the new /v2/profiles resource.")
    t.asst("- /v2/profiles field: display_name")
    explore(t, 5, "python", "billing/models", "Reading models.")
    t.user("Heads up: the audit log is not part of this change. Don't touch it.")
    c_audit = len(t.records)
    t.asst("- Audit log out of scope")
    t.boundary()
    explore(t, 6, "python", "billing/serializers", "Reading serializers.")
    t.user("Pagination: switch to cursor-based instead of offset. Offset breaks on large tables.")
    c_page = len(t.records)
    t.asst("- Pagination: cursor-based (opaque cursor param + limit)\n- Offset dropped for the new endpoints")
    t.user("And the field name: use displayName in camelCase after all, to match the mobile client. Not display_name.")
    c_name2 = len(t.records)
    t.asst("- Field is displayName (camelCase)")
    explore(t, 5, "python", "billing/tests", "Scanning tests.")
    shell(t, "pytest tests/test_profiles.py", "12 passed in 1.7s")
    c_tests = len(t.records)
    t.asst("- 12 profile tests pass\n- Next: write the OpenAPI entry for /v2/profiles and the cursor encoding spec")
    probes = [
        P("schema", "constraint", "Which endpoint's response schema must never change?", c_rules, any_of=["/v1/users"]),
        P("deps", "constraint", "Are new dependencies allowed?", c_rules, regex=r"(?i)^\W*no\b|\bnone\b|\bnot\b.{0,20}\ballowed\b", needle=r"no new dependencies"),
        P("style", "constraint", "What reply style did the user request?", c_rules, regex=r"(?i)terse|bullet"),
        P("pagination", "superseded", "Which pagination scheme is used for the new endpoints?", c_page, regex=r"(?i)cursor", stale=[r"(?i)\boffset\b(?!.*(drop|break|instead))"]),
        P("field", "superseded", "What is the final name of the profile display field?", c_name2, regex=r"displayName", stale=[r"display_name", r"fullName"]),
        P("audit", "negation", "Is the audit log part of this change?", c_audit, regex=r"(?i)^\W*no\b|\bnot part\b|\bout of scope\b|\bdon'?t touch\b"),
        P("why_cursor", "decision", "Why was cursor pagination chosen?", c_page, regex=r"(?i)\blarge tables?\b|offset breaks?"),
        P("why_camel", "decision", "Why was camelCase chosen for the field?", c_name2, regex=r"(?i)mobile"),
        P("tests", "tool_result", "How many profile tests passed?", c_tests, regex=r"\b12\b"),
        P("auth_scheme", "decoy", "Which OAuth scope name guards /v2/profiles?", 1, expect_unknown=True),
        P("next", "next_action", "What is the next step?", len(t.records), regex=r"(?is)openapi.*cursor|cursor.*openapi"),
    ]
    finish(os.path.join(CASES, "constraints-api-design"), "constraints-api-design", "superseded", t, probes,
           "Write the OpenAPI entry for /v2/profiles and the cursor encoding spec.",
           "API design with standing constraints, negations and two reversed decisions; terse reply style.")


# -------------------------------------------------------------------- narrative

def narrative_lighthouse():
    t = Transcript(88)
    t.scale = 3
    t.user("Let's co-write a short novel. Setting: the island of Skarn, 1894. Protagonist: Mara, a lighthouse keeper. Her sister is Ingrid. Third person, past tense, no graphic violence.")
    c_setup = 1
    t.asst("Chapter 1. Mara climbed the stair of the Skarn light for the nine hundredth time. " + t.prose(25))
    t.user("Small change: rename the protagonist from Mara to Marit Oyelaran everywhere from now on.")
    c_name = len(t.records)
    t.asst("Marit Oyelaran it is. " + t.prose(20))
    t.user("Chapter 2: the ship Halcyon Dray runs aground in fog on the shoal. The harbor-master, Teague, blames the lamp.")
    c_ship = len(t.records)
    t.asst("Chapter 2. The Halcyon Dray struck the shoal before dawn. Harbor-master Teague arrived by cutter and blamed the lamp. " + t.prose(30))
    for i in range(3):
        t.user(f"Continue the scene, beat {i + 1}.")
        t.asst(t.prose(32))
    t.user("Secret: the great lens has a hairline crack that Marit has hidden from everyone, even Ingrid. Marit also carries a brass compass engraved with the initials T.K., which belonged to her father.")
    c_secret = len(t.records)
    t.asst("Marit touched the compass engraved T.K. and thought of the cracked lens she had told no one about. " + t.prose(26))
    t.boundary()
    for i in range(4):
        t.user(f"Chapter 3, beat {i + 1}: keep going.")
        t.asst(t.prose(34))
    t.user("Plot turn: Ingrid arrives by the supply boat on the fourth day and notices the crack immediately. Marit confesses that she lied to the inspector in March.")
    c_turn = len(t.records)
    t.asst("Ingrid arrived on the fourth day and saw the crack at once. Marit confessed the lie she had told the inspector in March. " + t.prose(24))
    t.user("The tone should stay quiet and melancholy; no humor. Also Teague secretly owes money to the shipping line, and that is why he wants the lamp blamed.")
    c_teague = len(t.records)
    t.asst("Understood: quiet and melancholy. Teague's debt to the shipping line is the hidden motive. " + t.prose(20))
    for i in range(3):
        t.user(f"Chapter 4, beat {i + 1}: continue.")
        t.asst(t.prose(34))
    t.user("End the chapter on Marit lighting the lamp again despite the crack.")
    t.asst("Marit struck the match, and the cracked lens threw a thin, trembling beam across the dark water. Next chapter: the inspector's return and Teague's exposure.")
    probes = [
        P("island", "narrative", "What is the name of the island and the year?", c_setup, regex=r"(?is)Skarn.*1894|1894.*Skarn"),
        P("pov", "constraint", "What narrative person and tense were requested?", c_setup, regex=r"(?is)third.*past|past.*third"),
        P("name", "superseded", "What is the protagonist's full name now?", c_name, regex=r"Marit Oyelaran", stale=[r"\bMara\b"]),
        P("sister", "narrative", "Who is the protagonist's sister?", c_setup, any_of=["Ingrid"]),
        P("ship", "narrative", "What was the name of the ship that ran aground?", c_ship, any_of=["Halcyon Dray"]),
        P("villain", "narrative", "Who blames the lamp for the wreck?", c_ship, any_of=["Teague"]),
        P("compass", "narrative", "What initials are engraved on the compass?", c_secret, regex=r"T\.?K\.?"),
        P("secret", "narrative", "What flaw has the protagonist been hiding?", c_secret, regex=r"(?i)crack"),
        P("motive", "narrative", "Why does Teague want the lamp blamed?", c_teague, regex=r"(?i)debt|owes|shipping line"),
        P("confession", "narrative", "What did the protagonist confess to Ingrid?", c_turn, regex=r"(?i)\blied\b|\blie\b|\blying\b"),
        P("tone", "constraint", "What tone was requested?", c_teague, regex=r"(?i)quiet|melanchol"),
        P("dog", "decoy", "What is the name of Marit's dog?", 1, expect_unknown=True),
        P("next", "next_action", "What happens in the next chapter?", len(t.records), regex=r"(?is)inspector.*Teague|Teague.*inspector"),
    ]
    finish(os.path.join(CASES, "narrative-lighthouse"), "narrative-lighthouse", "narrative", t, probes,
           "Write the next chapter: the inspector returns and Teague is exposed.",
           "Collaborative novel with a character rename, a hidden secret and a hidden motive.")


# ------------------------------------------------------------------ behavior
# Text-only conversations whose last compaction is followed by one extra user message.
# `behavior.json` lists those messages and how to grade the reply (see behavior.py).

def turn_spec(tid, message, kind, social=False, **extra):
    spec = {"id": tid, "message": message, "kind": kind, "social": social}
    spec.update(extra)
    return spec


def chat(t, user, answer, bulk=14):
    t.user(user)
    t.asst(answer + " " + t.prose(bulk))


def behavior_greeting():
    t = Transcript(71)
    t.scale = 3
    t.user("Refactor the tokenizer in lexer.rs so scan_ident handles Unicode identifiers. Constraint: no new crates; keep scan_ident under 40 lines.")
    t.asst("Plan: replace the ASCII check in scan_ident with char::is_alphanumeric plus underscore, keep the function small. " + t.prose(14))
    for i in range(14):
        chat(t, f"Lexer detail {i + 1}: check the edge case for identifiers with combining marks.", f"Edge case {i + 1} reviewed.")
    t.boundary()
    t.user("Decision: we use char::is_alphanumeric rather than the unicode-xid crate, because the no-new-crates constraint rules the crate out.")
    c_decision = len(t.records)
    t.asst("Recorded: is_alphanumeric, not unicode-xid, because of the no-new-crates constraint. " + t.prose(14))
    for i in range(14):
        chat(t, f"Test idea {i + 1} for the lexer: try identifiers mixing scripts.", f"Test idea {i + 1} noted.")
    t.user("Run the lexer tests.")
    t.asst("lexer::tests::unicode_ident fails: expected Ident(\"naïve\") got Error at column 3. Next I will fix scan_ident to accept the combining mark, then rerun the tests and update docs/lexer.md. " + t.prose(10))
    probes = [P("decision", "decision", "Why was unicode-xid not used?", c_decision, regex=r"(?i)no.new.crates|constraint")]
    behavior = {"turns": [
        turn_spec("hi", "hi", "greeting", True),
        turn_spec("thanks", "thanks!", "greeting", True),
        turn_spec("nihao", "你好", "greeting", True),
        turn_spec("howdy", "hey, how's it going?", "greeting", True),
    ], "markers": r"(?i)lexer|scan_ident|unicode_ident|tokenizer|unicode-xid|docs/lexer|failing test|refactor",
        "max_chars": 300}
    finish(os.path.join(CASES, "behavior-greeting"), "behavior-greeting", "behavior", t, probes,
           "Fix scan_ident for combining marks, rerun the lexer tests, update docs/lexer.md.",
           "Unfinished lexer task; greetings after compaction must not resume it.",
           suites=["behavior"], behavior=behavior)


def behavior_unrelated():
    t = Transcript(72)
    t.scale = 3
    t.user("Write an importer for data/legacy/orders_2019.csv into SQLite. Never modify the original file; skipped rows go to rejects.csv.")
    t.asst("Plan: import from a copy, write rejects.csv for bad rows. " + t.prose(14))
    for i in range(14):
        chat(t, f"Importer detail {i + 1}: how should we treat the quantity column?", f"Quantity handling {i + 1}.")
    t.boundary()
    for i in range(14):
        chat(t, f"Importer check {i + 1}: validate customer ids.", f"Validation {i + 1}.")
    t.user("Status?")
    t.asst("The importer reads a copy of orders_2019.csv and writes rejects.csv. Remaining: add the customer_id index and document the rejects.csv format in the README. " + t.prose(10))
    probes = [P("rejects", "constraint", "Where do skipped rows go?", 1, regex=r"rejects\.csv")]
    behavior = {"turns": [
        turn_spec("capital", "What's the capital of Australia?", "unrelated", False, must_match=[r"(?i)canberra"]),
        turn_spec("ounces", "Quick one: how many ounces are in a pound?", "unrelated", False, must_match=[r"\b16\b"]),
        turn_spec("spelling", "How do you spell 'necessary'?", "unrelated", False, must_match=[r"(?i)necessary"]),
    ], "markers": r"(?i)importer|orders_2019|rejects\.csv|customer_id|sqlite|readme|legacy",
        "max_chars": 500}
    finish(os.path.join(CASES, "behavior-unrelated"), "behavior-unrelated", "behavior", t, probes,
           "Add the customer_id index and document rejects.csv.",
           "Unfinished importer task; an unrelated question after compaction is answered and the task is left alone.",
           suites=["behavior"], behavior=behavior)


def behavior_topic_return():
    t = Transcript(73)
    t.scale = 3
    t.user("Plan a trip to Lisbon with me. Dates: March 14 to March 19. Total budget 1800 euros. I have a peanut allergy. I want to stay in Alfama.")
    c_trip = 1
    t.asst("Noted: Lisbon, March 14-19, 1800 euro budget, peanut allergy, Alfama. " + t.prose(14))
    for i in range(10):
        chat(t, f"Trip detail {i + 1}: what about getting around the city?", f"Transit option {i + 1}.")
    t.user("Park the trip for now. Let's work on the rate limiter.")
    t.asst("Trip parked. " + t.prose(6))
    t.boundary()
    t.user("Rate limiter requirements: token bucket, free tier 60 requests per minute, paid tier 600. The remaining-tokens header is X-RateLimit-Remaining.")
    c_rl = len(t.records)
    t.asst("Recorded: token bucket, free 60/min, paid 600/min, header X-RateLimit-Remaining. " + t.prose(14))
    for i in range(12):
        chat(t, f"Limiter detail {i + 1}: how do we refill the bucket?", f"Refill option {i + 1}.")
    t.user("Decision: refill on request using the monotonic clock, because wall-clock jumps broke the burst test.")
    c_clock = len(t.records)
    t.asst("Decided: refill on request with the monotonic clock, since wall-clock jumps broke the burst test. " + t.prose(12))
    probes = [
        P("dates", "parked_topic", "What dates is the Lisbon trip?", c_trip, regex=r"(?i)March 14"),
        P("tier", "identifier", "How many requests per minute for the free tier?", c_rl, regex=r"\b60\b"),
        P("clock", "decision", "Why was the monotonic clock chosen for refills?", c_clock, regex=r"(?i)wall.clock"),
    ]
    behavior = {"turns": [
        turn_spec("back_to_trip", "Let's get back to the Lisbon trip. Remind me: what dates, total budget, food allergy and neighborhood did we settle on?", "return", False,
                  facts=[r"(?i)March 14", r"1,?800", r"(?i)peanut", r"(?i)Alfama"]),
        turn_spec("back_to_limiter", "Actually, back to the rate limiter: what are the two tier limits, the header name, and how do we refill?", "return", False,
                  facts=[r"\b60\b", r"\b600\b", r"X-RateLimit-Remaining", r"(?i)monotonic"]),
    ], "markers": r"(?i)$^", "max_chars": 1500}
    finish(os.path.join(CASES, "behavior-topic-return"), "behavior-topic-return", "behavior", t, probes,
           "Return to whichever topic the user names.",
           "Topic A (trip) then topic B (limiter), each followed by a compaction; the user returns to either topic.",
           suites=["behavior"], behavior=behavior)


def behavior_continue():
    t = Transcript(74)
    t.scale = 3
    t.user("Help me adapt a pancake recipe: I need a buttermilk substitute because I am lactose intolerant.")
    t.asst("Use 1 cup oat milk plus 1 tablespoon lemon juice per cup of buttermilk. " + t.prose(14))
    for i in range(10):
        chat(t, f"Pancake tweak {i + 1}: any other change?", f"Tweak {i + 1}.")
    t.user("Park the recipe. New task: the invoice total in totals.py is off by a cent sometimes.")
    t.asst("Recipe parked. Looking at totals.py. " + t.prose(10))
    t.boundary()
    for i in range(14):
        chat(t, f"Invoice detail {i + 1}: check how round_cents is used.", f"round_cents usage {i + 1}.")
    t.user("Found it: round_cents uses round() which rounds half to even in some paths and half up in others. Use decimal ROUND_HALF_UP everywhere.")
    t.asst("Plan: switch round_cents in totals.py to decimal ROUND_HALF_UP. Next I will make that change and then add a regression test test_round_half_up for 2.675. " + t.prose(10))
    probes = [P("mode", "decision", "Which rounding mode was chosen?", len(t.records) - 1, regex=r"(?i)ROUND_HALF_UP")]
    marker = r"(?i)round_cents|totals\.py|ROUND_HALF_UP|half.up|regression test|invoice"
    behavior = {"turns": [
        turn_spec("continue", "continue", "continue", False, resume=marker, wrong_topic=r"(?i)buttermilk|pancake|oat milk"),
        turn_spec("go_on", "ok go on", "continue", False, resume=marker, wrong_topic=r"(?i)buttermilk|pancake|oat milk"),
        turn_spec("keep_going", "keep going", "continue", False, resume=marker, wrong_topic=r"(?i)buttermilk|pancake|oat milk"),
    ], "markers": r"(?i)$^", "max_chars": 4000}
    finish(os.path.join(CASES, "behavior-continue"), "behavior-continue", "behavior", t, probes,
           "Change round_cents to ROUND_HALF_UP and add test_round_half_up.",
           "A parked recipe and a current invoice task; a vague continue must resume the latest task.",
           suites=["behavior"], behavior=behavior)


def main():
    for build in (coding_rate_limiter, coding_csv_migration, coding_rust_parser, parked_trip_bugfix,
                  parked_budget_recipe, superseded_deploy_plan, constraints_api_design, narrative_lighthouse,
                  behavior_greeting, behavior_unrelated, behavior_topic_return, behavior_continue):
        build()


if __name__ == "__main__":
    main()
