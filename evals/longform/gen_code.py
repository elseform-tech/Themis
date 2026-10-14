"""Code long-form cases: an API spec, a build/test log and a multi-session refactor history."""

import re

from evals.compaction.fixturekit import python_module

from .gen_narrative import _ends, _spread
from .kit import Case

RESOURCES = ("ledgers shipments tenants webhooks exports quotas sessions invoices carriers "
             "pallets manifests depots tariffs audits routes").split()
FIELD_WORDS = ("memo label region cursor window ttl batch retry digest origin vessel carrier "
               "lane bin zone cohort epoch sink probe shard stamp tier hold").split()
PURPOSES = ["Free-form text shown on statements", "Delay between delivery attempts in seconds",
            "Identifier of the owning tenant", "Upper bound on rows returned per page",
            "Region the record is pinned to", "Seconds before an idle record is archived",
            "Opaque cursor for the next page", "Checksum used to detect duplicate uploads"]


def fileline(path, line):
    """Regex for a `file:line` answer in the usual spellings (`a/b.rs:12`, `b.rs line 12`, `b.rs, line 12`)."""
    base = re.escape(path.rsplit("/", 1)[-1])
    return rf"(?<![\w-]){base}\W{{1,12}}(?:line\W{{0,3}})?{line}(?![\w-]|\.\d)"


# ---------------------------------------------------------------- API spec

def api_spec(root):
    case = Case("longform-api-spec", "code", 74,
                "Long synthetic REST API design spec (~80 KB) with exact field names, limits, error codes and amendments.")
    rng, uniq, t = case.rng, case.unique, case.t
    endpoints = []
    parts, current = [], []
    for res in RESOURCES:
        for verb, method, path in (("create", "POST", f"/v3/{res}"), ("get", "GET", f"/v3/{res}/{{id}}"),
                                   ("list", "GET", f"/v3/{res}"), ("update", "PATCH", f"/v3/{res}/{{id}}")):
            rate, window = uniq.number(41, 970), rng.choice([10, 30, 60, 120])
            scope = f"{res}.{verb}.{uniq.code('s', 4)}"
            error = f"ERR_{res.upper()}_{verb.upper()}_{rng.randint(1000, 9999)}"
            error = uniq.take(lambda e=error: e)
            status = rng.choice([400, 403, 404, 409, 422, 429])
            fields = []
            for _ in range(5 if verb != "get" else 2):
                name = uniq.take(lambda: f"{rng.choice(FIELD_WORDS)}_{rng.choice(FIELD_WORDS)}_{rng.choice(['id','key','ms','n','ref'])}")
                fields.append(dict(name=name, max=uniq.number(21, 4090), purpose=rng.choice(PURPOSES),
                                   default=uniq.number(3, 98) if verb == "list" else None))
            lines = [f"### {method} {path}  ({verb})", f"Required scope: `{scope}`.",
                     f"Rate limit: {rate} requests per {window} seconds per tenant.", "Request fields:"]
            for f in fields:
                default = f" Default {f['default']}." if f["default"] else ""
                lines.append(f"  - `{f['name']}` (string, max {f['max']} chars): {f['purpose']}.{default}")
            lines += ["Errors:", f"  - HTTP {status} `{error}`: the {res[:-1]} is not in a state that allows {verb}.",
                      "Behaviour: the server applies the change atomically and replays the response for repeated "
                      f"requests that carry the same idempotency key. {_prose(rng)}"]
            endpoints.append(dict(res=res, verb=verb, method=method, path=path, rate=rate, window=window,
                                  scope=scope, error=error, status=status, fields=fields))
            current.append("\n".join(lines))
        if len(current) >= 8:
            parts.append("\n\n".join(current))
            current = []
    if current:
        parts.append("\n\n".join(current))
    header = uniq.take(lambda: f"X-Themis-{rng.choice(FIELD_WORDS).title()}-{rng.choice(FIELD_WORDS).title()}-{rng.randint(10, 99)}")
    rule = uniq.phrase(7)
    rule = rule[0].upper() + rule[1:]
    owners = []
    for i, text in enumerate(parts):
        extra = ""
        if i == 0:
            extra = f"\n\nGlobal rules. Every mutating request must carry the `{header}` header. The spec states: \"{rule}.\""
        owners.append(t.user(f"Design spec part {i + 1} of {len(parts) + 1} for the Harbour API (v3). Keep exact field names, limits and error codes; "
                             f"I will quiz you later.\n\n{text}{extra}"))
        t.asst(f"Part {i + 1} recorded. I will keep the exact names, limits and codes available.")
        if i % 1 == 0 and i >= 1:
            t.boundary()
    # Final amendments part supersedes three earlier limits.
    amended = [endpoints[3], endpoints[17], endpoints[30]]
    lines = ["Amendments (these supersede the earlier text):"]
    for e in amended:
        e["new_rate"] = uniq.number(41, 970)
        lines.append(f"- {e['method']} {e['path']} ({e['verb']}): the rate limit is now {e['new_rate']} requests per {e['window']} seconds per tenant.")
    amend_record = t.user(f"Design spec part {len(parts) + 1} of {len(parts) + 1}.\n\n" + "\n".join(lines))
    t.asst("Amendments recorded; the new rate limits replace the earlier ones.")

    def owner(e):
        return owners[(endpoints.index(e)) // 8 if endpoints.index(e) // 8 < len(owners) else -1]

    for e in _spread(endpoints[1::4] + endpoints[2::4], 3):
        f = e["fields"][0]
        case.probe("detail", f"In the Harbour API spec, what is the maximum length (in characters) of the `{f['name']}` field of {e['method']} {e['path']} ({e['verb']})?",
                   owner(e), answer=str(f["max"]), evidence=rf"`{re.escape(f['name'])}` \(string, max {f['max']} chars\)")
    for e in _spread(endpoints[0::4], 2):
        case.probe("detail", f"What error code does {e['method']} {e['path']} ({e['verb']}) return on HTTP {e['status']}?",
                   owner(e), answer=e["error"])
    for e in _spread(endpoints[2::4], 2):
        case.probe("detail", f"Which OAuth scope does the {e['verb']} operation on {e['path']} require?",
                   owner(e), answer=e["scope"])
    for e in amended[:3]:
        case.probe("retraction", f"What is the current rate limit (requests) for {e['method']} {e['path']} ({e['verb']}) after the amendments?",
                   amend_record, answer=str(e["new_rate"]), stale=[str(e["rate"])])
    case.probe("quote", "What is the exact rule sentence quoted in the global rules of the spec? Quote it verbatim.", owners[0], exact=rule)
    case.probe("detail", "Which HTTP header must every mutating request carry?", owners[0], answer=header)
    case.probe("decoy", "What is the maximum payload size in megabytes accepted by the Harbour API gateway?", [],
               absent=[r"\bmegabytes?\b", r"\bMB\b", r"\bpayload size\b"])
    case.probe("decoy", "Which TLS cipher suite does the Harbour API require?", [], absent=[r"\bcipher\b", r"\bTLS\b"])
    case.finish(root, _ends(t))
    return case


def _prose(rng):
    bank = ["Clients should treat the response as authoritative.", "Retries use exponential backoff with jitter.",
            "The audit trail records the actor and the request identifier.", "Callers must not rely on ordering between pages.",
            "Soft-deleted records remain readable for the retention period.", "The server never reveals whether a tenant exists to outsiders.",
            "Bulk operations are rejected when they cross a region boundary.", "Timestamps are always UTC and carry millisecond precision."]
    return " ".join(rng.sample(bank, 6))


# ---------------------------------------------------------------- build / test log

def build_log(root):
    case = Case("longform-build-log", "code", 75,
                "Eight shards of noisy cargo test output (~110 KB) with one failing test buried in shard 5.")
    rng, uniq, t = case.rng, case.unique, case.t
    crates = ["store", "net", "sched", "auth", "index", "codec", "metrics", "cli"]
    modules = ("cache queue lease shard wal fence merge scan plan ring route frame token").split()

    def test_name(crate):
        return uniq.take(lambda: f"{crate}::{rng.choice(modules)}::tests::{rng.choice(['merge','scan','flush','split','probe','rewind','seal'])}_{rng.choice(['l0','l1','l2','l3','cold','warm','tail','head'])}_{rng.choice(['overlap','empty','dense','sparse','retry','wrap'])}")

    def noise(crate, count):
        out = []
        for _ in range(count):
            roll = rng.random()
            if roll < 0.78:
                out.append(f"test {test_name(crate)} ... ok")
            elif roll < 0.88:
                line = rng.randint(10, 400)
                out.append(f"warning: unused variable: `{rng.choice(modules)}_{rng.randint(1, 99)}`\n  --> crates/{crate}/src/{rng.choice(modules)}.rs:{line}:{rng.randint(5, 30)}")
            elif roll < 0.95:
                out.append(f"   Compiling {crate}-{rng.choice(modules)} v0.{rng.randint(1, 9)}.{rng.randint(0, 20)} (/work/crates/{crate})")
            else:
                out.append(f"test {test_name(crate)} ... ignored, needs a running broker")
        return "\n".join(out)

    rustc = uniq.take(lambda: f"rustc 1.{rng.randint(80, 99)}.0 ({rng.randint(10**8, 10**9 - 1):x} 2026-0{rng.randint(1, 9)}-{rng.randint(10, 28)})")
    owner, facts = {}, {}
    t.user("Run the whole workspace test suite shard by shard with `cargo test --no-fail-fast` and keep the raw output; "
           "I will ask about the details afterwards.")
    shard_text = {}
    for shard, crate in enumerate(crates, 1):
        head = f"$ cargo test -p {crate} --no-fail-fast\n{rustc if shard == 1 else ''}".rstrip()
        body = noise(crate, 150)
        passed, ignored = uniq.number(120, 990), uniq.number(2, 29)
        failed = 0
        tail = ""
        if shard == 5:
            failed = 1
            name = test_name(crate)
            loc = f"crates/{crate}/src/{rng.choice(modules)}_merge.rs"
            line, col = uniq.number(300, 990), rng.randint(9, 40)
            left, right = uniq.number(2100, 9900), uniq.number(2100, 9900)
            message = uniq.phrase(5)
            facts["fail"] = dict(name=name, loc=loc, line=line, col=col, left=left, right=right, message=message,
                                 crate=crate, ignored=ignored)
            tail = (f"\nfailures:\n\n---- {name} stdout ----\n"
                    f"thread '{name}' panicked at {loc}:{line}:{col}:\n"
                    f"assertion `left == right` failed: {message}\n  left: {left}\n right: {right}\n"
                    "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\n"
                    f"failures:\n    {name}\n")
        if shard == 2:
            flaky = test_name(crate)
            body += f"\ntest {flaky} ... FAILED\nretrying {flaky} (attempt 2 of 3) ... ok"
            facts["flaky"] = dict(name=flaky)
        if shard == 3:
            warnings = uniq.number(31, 97)
            body += f"\nwarning: `{crate}` (lib test) generated {warnings} warnings"
            facts["warn"] = dict(count=warnings, crate=crate)
        result = ("FAILED" if failed else "ok")
        tail += (f"\ntest result: {result}. {passed} passed; {failed} failed; {ignored} ignored; 0 measured; "
                 f"0 filtered out; finished in {rng.randint(3, 80)}.{rng.randint(10, 99)}s\n")
        text = f"{head}\n{body}{tail}"
        facts.setdefault("shards", {})[shard] = dict(passed=passed, ignored=ignored, crate=crate)
        shard_text[shard] = text
        owner[shard] = t.call("shell", {"command": "cargo", "args": ["test", "-p", crate, "--no-fail-fast"]}, text,
                              text=f"Running shard {shard} of {len(crates)} ({crate}).")
        t.asst(f"Shard {shard} ({crate}) captured.")
        if shard % 1 == 0 and shard >= 2:
            t.boundary()
    t.asst("All shards ran. Ask me about any detail of the output.")
    f = facts["fail"]
    fail_ev = rf"panicked at {re.escape(f['loc'])}:{f['line']}:{f['col']}"
    case.probe("detail", "Which test failed in the workspace test run?", owner[5], answer=f["name"])
    case.probe("detail", "At which file, line and column did the failing test panic? Answer as path:line:col.", owner[5],
               regex=fileline(f["loc"], f["line"]) + rf"\W{{0,3}}{f['col']}?", evidence=fail_ev,
               text=f"{f['loc']}:{f['line']}:{f['col']}")
    case.probe("detail", "In the failing assertion, what was the `left` value?", owner[5], answer=str(f["left"]),
               evidence=rf"left: {f['left']}\n right: {f['right']}")
    case.probe("detail", "In the failing assertion, what was the `right` value?", owner[5], answer=str(f["right"]),
               evidence=rf"left: {f['left']}\n right: {f['right']}")
    case.probe("detail", "Which crate's shard contained the failing test?", owner[5], regex=r"(?<![\w-])" + re.escape(f["crate"]) + r"(?![\w-])", text=f["crate"],
               evidence=rf"\$ cargo test -p {f['crate']} --no-fail-fast(?s:.*){re.escape(f['name'])} stdout")
    case.probe("detail", "How many tests were ignored in the shard that had the failure?", owner[5], answer=str(f["ignored"]),
               evidence=rf"failed; {f['ignored']} ignored")
    case.probe("detail", "Which test failed once but passed on retry?", owner[2], answer=facts["flaky"]["name"])
    case.probe("detail", "How many warnings did the lib test of the shard-3 crate generate?", owner[3], answer=str(facts["warn"]["count"]),
               evidence=rf"generated {facts['warn']['count']} warnings")
    case.probe("detail", "Which rustc version string was printed at the start of the first shard?", owner[1], answer=rustc)
    case.probe("quote", "What is the exact message text of the failing assertion (after 'left == right failed:')? Quote it verbatim.",
               owner[5], exact=f["message"])
    case.probe("decoy", "What was the segfault address reported in the test log?", [], absent=[r"segfault", r"SIGSEGV"])
    case.probe("decoy", "Which CI runner image executed these shards?", [], absent=[r"runner image", r"ubuntu-", r"macos-", r"windows-"])
    case.finish(root, _ends(t))
    return case


# ---------------------------------------------------------------- refactor history

def refactor(root):
    case = Case("longform-refactor", "code", 76,
                "Five-session refactor history (tool calls over ~20 files) with a bug introduced at one file:line and fixed at another.")
    rng, uniq, t = case.rng, case.unique, case.t
    pkg = ["billing/checkout", "billing/invoice", "pricing/rounding", "pricing/tiers", "pricing/regions",
           "orders/store", "orders/events", "orders/export", "catalog/index", "catalog/sync", "users/accounts",
           "users/sessions", "reports/nightly", "reports/summary", "jobs/queue", "jobs/retry", "api/routes",
           "api/schemas", "util/dates", "util/money"]
    files = {f"src/{p}.py": python_module(rng, 9).splitlines() for p in pkg}
    BUG = "src/pricing/rounding.py"
    lines = files[BUG]
    while len(lines) < 240:
        lines += python_module(rng, 4).splitlines()
    lines = lines[:240]
    fn = "apply_tier_discounts"
    block = ["", f"def {fn}(subtotal, tiers):", '    """Apply the first matching tier discount to the subtotal."""', "    total = subtotal",
             "    for step in range(0, len(tiers)):", "        if subtotal >= tiers[step].floor:", "            total = subtotal - tiers[step].discount",
             "            break", "    return total", ""]
    lines[182:192] = block  # 1-based: `def` on 184, loop header on 187
    assert lines[186].lstrip().startswith("for step in range(0,"), lines[186]
    files[BUG] = lines
    bug_line = 187

    def numbered(path, start=1, end=None):
        body = files[path][start - 1:end]
        return "\n".join(f"{n:>5}\t{text}" for n, text in enumerate(body, start))

    def read(path, start=1, end=None):
        return t.call("read_file", {"file_path": path}, numbered(path, start, end))

    def patch(path, before, after, line, note):
        out = (f"patched {path}: 1 hunk, first changed line {line}\n--- a/{path}\n+++ b/{path}\n@@ -{line - 3},7 +{line - 3},7 @@\n"
               f" {files[path][line - 4]}\n {files[path][line - 3]}\n {files[path][line - 2]}\n-{before}\n+{after}\n"
               f" {files[path][line]}\n {files[path][line + 1]}\n {files[path][line + 2]}\n")
        files[path][line - 1] = after
        return t.call("apply_patch", {"path": path, "note": note}, out, text=note)

    facts = {}
    # --- session 1: rename compute_total across checkout/invoice/store
    t.user("Session 1. Refactor pricing: rename `compute_total` to a more specific name in each caller module, and keep behaviour identical.")
    for path in ("src/billing/checkout.py", "src/billing/invoice.py", "src/orders/store.py"):
        read(path)
        new = uniq.take(lambda: f"price_{rng.choice(['order','basket','cart','parcel'])}_{rng.choice(['net','gross','final','base'])}_{rng.randint(10, 99)}")
        owner = t.call("apply_patch", {"path": path, "rename": ["compute_total", new]},
                       f"patched {path}: renamed compute_total -> {new} (4 references)", text=f"Renaming in {path}.")
        facts[path] = dict(new=new, record=owner)
    msg1 = uniq.phrase(6)
    msg1 = "refactor: " + msg1
    facts["commit1"] = dict(msg=msg1, record=t.call("shell", {"command": "git", "args": ["commit", "-am", msg1]},
                                                    f"[refactor/pricing {rng.randint(10**6, 10**7 - 1):x}] {msg1}\n 3 files changed, 12 insertions(+), 12 deletions(-)"))
    ran = uniq.number(211, 480)
    t.call("shell", {"command": "pytest", "args": ["-q"]}, f"{ran} passed in {rng.randint(3, 9)}.{rng.randint(10, 99)}s")
    t.asst("Session 1 done: the rename is committed and the suite passes.")
    t.boundary()
    # --- session 2: introduce the bug in rounding.py:187
    t.user("Session 2. Make tier discounts skip the entry-level tier, we only want to discount from the second tier up.")
    t.call("search_file", {"pattern": "apply_tier_discounts"}, f"{BUG}:184:def {fn}(subtotal, tiers):\nsrc/billing/checkout.py:58:    total = {fn}(subtotal, tiers)")
    read(BUG, 170, 200)
    buggy = "    for step in range(1, len(tiers)):"
    owner_bug = patch(BUG, "    for step in range(0, len(tiers)):", buggy, bug_line, "Skip tier 0 as requested.")
    facts["bug"] = dict(record=owner_bug, line=bug_line, expr=buggy.strip())
    ran2 = uniq.number(211, 480)
    t.call("shell", {"command": "pytest", "args": ["-q"]}, f"{ran2} passed in {rng.randint(3, 9)}.{rng.randint(10, 99)}s")
    t.asst("Session 2 done: tiers now start at the second entry and the suite is green.")
    t.boundary()
    # --- session 3: unrelated work that shifts the lines
    t.user("Session 3. Add region support: import the region table at the top of rounding.py and log the region in the nightly report.")
    for path in ("src/pricing/regions.py", "src/reports/nightly.py", "src/api/schemas.py"):
        read(path)
    import_lines = ["import decimal  # region rounding", "from .regions import REGION_TABLE", "REGION_DEFAULT = 'eu'"]
    files[BUG][0:0] = import_lines
    t.call("apply_patch", {"path": BUG, "insert_at": 1},
                 f"patched {BUG}: 1 hunk, first changed line 1 (+3 lines)\n@@ -1,3 +1,6 @@\n+{import_lines[0]}\n+{import_lines[1]}\n+{import_lines[2]}")
    for path in ("src/orders/events.py", "src/orders/export.py", "src/catalog/index.py", "src/catalog/sync.py"):
        read(path)
    t.asst("Session 3 done: regions are wired in; rounding.py grew by three lines.")
    t.boundary()
    # --- session 4: failing test found
    t.user("Session 4. The nightly report shows wrong totals for first-tier orders. Find out why.")
    test = uniq.take(lambda: f"tests/test_tiers.py::test_{rng.choice(['entry','first','base'])}_tier_{rng.choice(['discount','floor','rounding'])}_{rng.randint(10, 99)}")
    want, got = uniq.number(2100, 9900), uniq.number(2100, 9900)
    msg = uniq.phrase(5)
    fail_out = (f"============ FAILURES ============\n___ {test.split('::')[1]} ___\n\n    def {test.split('::')[1]}():\n"
                f">       assert price == {want}, \"{msg}\"\nE       AssertionError: {msg}\nE       assert {got} == {want}\n\n"
                f"{test} FAILED\n=== 1 failed, {int(ran2) - 1} passed in {rng.randint(3, 9)}.{rng.randint(10, 99)}s ===")
    facts["test"] = dict(name=test, want=want, got=got, msg=msg, record=t.call("shell", {"command": "pytest", "args": ["-q", "tests/test_tiers.py"]}, fail_out))
    for path in ("src/pricing/tiers.py", "src/reports/summary.py"):
        read(path)
    grep_out = f"{BUG}:{bug_line + 3}:{buggy}"
    t.call("shell", {"command": "grep", "args": ["-n", "range(1, len(tiers))", BUG]}, grep_out)
    t.asst("The loop in apply_tier_discounts starts at index 1, which skips the first tier entirely.")
    t.boundary()
    # --- session 5: fix
    t.user("Session 5. Fix it so entry-level orders get their discount, but keep the second-tier-and-up rule documented.")
    read(BUG, 180, 200)
    fixed = "    for step in range(0, len(tiers)):  # tier 0 is the entry tier and must be checked"
    fix_line = bug_line + 3
    owner_fix = patch(BUG, buggy, fixed, fix_line, "Restore tier 0.")
    facts["fix"] = dict(record=owner_fix, line=fix_line)
    ran3 = uniq.number(211, 480)
    facts["final"] = dict(count=ran3, record=t.call("shell", {"command": "pytest", "args": ["-q"]}, f"{ran3} passed in {rng.randint(3, 9)}.{rng.randint(10, 99)}s"))
    msg5 = "fix: " + uniq.phrase(6)
    facts["commit5"] = dict(msg=msg5, record=t.call("shell", {"command": "git", "args": ["commit", "-am", msg5]},
                                                    f"[refactor/pricing {rng.randint(10**6, 10**7 - 1):x}] {msg5}\n 1 file changed, 1 insertion(+), 1 deletion(-)"))
    t.asst("Session 5 done: the bug is fixed and the suite passes.")
    t.boundary()

    b, fx = facts["bug"], facts["fix"]
    case.probe("detail", "At which file and line was the tier-skipping bug introduced during the refactor? Answer as path:line.",
               b["record"], regex=fileline(BUG, bug_line), text=f"{BUG}:{bug_line}",
               evidence=rf"first changed line {bug_line}\n--- a/{re.escape(BUG)}", stale=None)
    case.probe("detail", "What was the exact buggy loop header that was introduced in session 2?", b["record"],
               regex=r"range\(\s*1\s*,\s*len\(tiers\)\s*\)", text=buggy.strip(), evidence=r"\+" + re.escape(buggy))
    case.probe("detail", "At which line of rounding.py (after the later import additions) was the bug finally fixed?",
               fx["record"], regex=fileline(BUG, fx["line"]), text=f"{BUG}:{fx['line']}", evidence=rf"first changed line {fx['line']}\n--- a/{re.escape(BUG)}")
    case.probe("detail", "Which test caught the tier bug, and what was its full pytest id?", facts["test"]["record"], answer=test)
    case.probe("detail", "In the failing tier test, what total did the assertion expect?", facts["test"]["record"], answer=str(want),
               evidence=rf"assert {got} == {want}")
    case.probe("detail", "In the failing tier test, what total did the code actually produce?", facts["test"]["record"], answer=str(got),
               evidence=rf"assert {got} == {want}")
    case.probe("detail", f"What did `compute_total` get renamed to in {pkg_path('billing/invoice')}?", facts["src/billing/invoice.py"]["record"],
               answer=facts["src/billing/invoice.py"]["new"])
    case.probe("detail", f"What did `compute_total` get renamed to in {pkg_path('orders/store')}?", facts["src/orders/store.py"]["record"],
               answer=facts["src/orders/store.py"]["new"])
    case.probe("detail", "How many tests passed in the final full pytest run of the last session?", facts["final"]["record"],
               answer=str(ran3), evidence=rf"(?m)^{ran3} passed")
    case.probe("quote", "What was the exact commit message of the session 1 refactor commit? Quote it verbatim.",
               facts["commit1"]["record"], exact=facts["commit1"]["msg"])
    case.probe("quote", "What was the exact assertion message in the failing tier test? Quote it verbatim.",
               facts["test"]["record"], exact=msg)
    case.probe("decoy", "Which database migration was applied during the refactor?", [], absent=[r"\bmigration\b", r"\balembic\b"])
    case.probe("decoy", "What was the CI pipeline run id of the final green build?", [], absent=[r"\bpipeline\b", r"\bCI run\b"])
    case.finish(root, _ends(t))
    return case


def pkg_path(p):
    return f"src/{p}.py"
