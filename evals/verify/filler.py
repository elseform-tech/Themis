"""The parts of a Ledgerly project that vary by seed: docs, sample data, a demo store and a long
application log. They make the project realistic in size (too big for a small context budget)
and give the scenarios their data; none of it changes the code.
"""

import json
import os

FIRST = ["Anna", "Jonas", "Lea", "Paul", "Marie", "Felix", "Clara", "Noah", "Ida", "Emil", "Lina", "Theo"]
LAST = ["Hartmann", "Roux", "Visser", "Gruber", "Lambert", "de Vries", "Weber", "Moreau", "Bakker", "Huber"]
FIRMS = ["GmbH", "SARL", "B.V.", "KG", "& Co", "Studio", "Werkstatt", "Atelier"]
COUNTRIES = ["DE", "FR", "NL", "AT"]
PRODUCTS = [("PEN", "Fountain pen", 250), ("BOOK", "Notebook A5", 1200), ("INK", "Ink bottle, blue", 435),
            ("CLIP", "Paper clips (100)", 199), ("FOLD", "Folder, kraft", 115), ("DESK", "Desk pad", 2999),
            ("GIFT", "Gift card", 5000), ("TAPE", "Washi tape", 329), ("CARD", "Greeting card", 285)]
# Prices that a float conversion truncates a cent short (19.99 * 100 = 1998.999...).
TRICKY_PRICES = ["19.99", "4.35", "0.29", "1.15", "2.85", "3.29", "8.95", "0.57"]
PLAIN_PRICES = ["2.50", "12.00", "29.99", "1.25", "5.00", "7.50", "9.90", "15.00", "0.50"]


def write(project, rng, planted):
    os.makedirs(os.path.join(project, "data"), exist_ok=True)
    os.makedirs(os.path.join(project, "logs"), exist_ok=True)
    os.makedirs(os.path.join(project, "docs", "decisions"), exist_ok=True)
    customers = _customers(rng)
    _write_json(project, "data/demo_store.json", _demo_store(rng, customers))
    _write_json(project, "data/sample_invoices.json", sample_invoices(rng, customers))
    with open(os.path.join(project, "data", "rates.csv"), "w", encoding="utf-8") as handle:
        handle.write("country,percent\nDE,19\nFR,20\nNL,21\nAT,20\n")
    with open(os.path.join(project, "logs", "app.log"), "w", encoding="utf-8") as handle:
        handle.write(_log(rng, customers))
    for name, text in docs(rng, planted).items():
        with open(os.path.join(project, name), "w", encoding="utf-8") as handle:
            handle.write(text)


def _write_json(project, name, data):
    with open(os.path.join(project, name), "w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=1)


def _customers(rng):
    people = []
    for index in range(12):
        name = f"{rng.choice(FIRST)} {rng.choice(LAST)} {rng.choice(FIRMS)}"
        people.append({"id": f"C{index + 101}", "name": name, "country": rng.choice(COUNTRIES),
                       "terms_days": rng.choice([14, 14, 30, 30, 45]), "email": f"billing{index + 101}@example.com"})
    return people


def _demo_store(rng, customers):
    """A store to run the CLI and the API server against (schema 2: valid with or without codes)."""
    invoices = []
    for number in range(1, 19):
        customer = rng.choice(customers)
        day = rng.randint(1, 27)
        month = rng.choice([1, 2, 3])
        lines = [{"sku": sku, "quantity": rng.randint(1, 6), "unit_cents": cents}
                 for sku, _name, cents in rng.sample(PRODUCTS, rng.randint(1, 3))]
        status = rng.choice(["open", "open", "open", "paid", "void"])
        paid = _total(lines, customer["country"]) if status == "paid" else 0
        invoices.append({"id": f"INV-2026-{number:05d}", "customer_id": customer["id"],
                         "issued": f"2026-{month:02d}-{day:02d}", "lines": lines, "status": status,
                         "paid_cents": paid, "created_at": f"2026-{month:02d}-{day:02d}T09:{rng.randint(10, 59)}:00"})
    return {"schema": 2, "customers": customers,
            "products": [{"sku": sku, "name": name, "unit_cents": cents, "taxable": sku != "GIFT"}
                         for sku, name, cents in PRODUCTS],
            "invoices": invoices,
            "stock": [{"sku": sku, "on_hand": rng.randint(40, 400), "reserved": 0} for sku, _n, _c in PRODUCTS]}


def _total(lines, country):
    """What `ledgerly.services.pricing.total` gives for these lines (no discount), for paid invoices."""
    from decimal import ROUND_HALF_UP, Decimal

    rate = {"DE": 19, "FR": 20, "NL": 21, "AT": 20}[country]
    subtotal = sum(line["quantity"] * line["unit_cents"] for line in lines)
    taxable = sum(line["quantity"] * line["unit_cents"] for line in lines if line["sku"] != "GIFT")
    tax = int((Decimal(taxable) * rate / 100).quantize(Decimal("1"), rounding=ROUND_HALF_UP))
    return subtotal + tax


def sample_invoices(rng, customers):
    """Till records for `import`: plain, slash and offset-timestamp dates; some tricky prices."""
    records = []
    for number in range(1, 41):
        customer = rng.choice(customers)
        day, month = rng.randint(1, 27), rng.choice([2, 3])
        style = number % 5
        if style == 0:
            hour = rng.choice([22, 23])
            date = f"2026-{month:02d}-{day:02d}T{hour}:{rng.randint(10, 59)}:00-0{rng.choice([4, 5, 7])}:00"
        elif style == 1:
            date = f"{day:02d}/{month:02d}/2026"
        else:
            date = f"2026-{month:02d}-{day:02d}"
        prices = TRICKY_PRICES if number % 3 == 0 else PLAIN_PRICES
        lines = [{"sku": sku, "qty": rng.randint(1, 5), "price": rng.choice(prices)}
                 for sku, _name, _cents in rng.sample(PRODUCTS, rng.randint(1, 3))]
        records.append({"id": f"T-{number:04d}", "customer": customer["id"], "date": date, "lines": lines})
    return records


def _log(rng, customers):
    """About 150k characters of realistic INFO/WARN lines: requests, imports, payments, jobs."""
    lines = []
    paths = ["/api/invoices", "/api/invoices?status=open", "/api/invoices/INV-2026-00{n:03d}/payments", "/index.html"]
    for minute in range(1600):
        stamp = f"2026-03-{1 + minute // 96:02d}T{(minute // 4) % 24:02d}:{(minute * 15) % 60:02d}:{rng.randint(0, 59):02d}Z"
        kind = rng.random()
        customer = rng.choice(customers)
        if kind < 0.55:
            path = rng.choice(paths).format(n=rng.randint(1, 120))
            lines.append(f"{stamp} INFO  api      {rng.choice(['GET', 'GET', 'POST'])} {path} -> 200 in {rng.randint(3, 90)}ms")
        elif kind < 0.75:
            lines.append(f"{stamp} INFO  billing  issued INV-2026-{rng.randint(1, 900):05d} for {customer['id']} "
                         f"({rng.randint(1, 6)} lines)")
        elif kind < 0.88:
            lines.append(f"{stamp} INFO  payments recorded {rng.randint(100, 40000)} cents on INV-2026-{rng.randint(1, 900):05d}")
        elif kind < 0.96:
            lines.append(f"{stamp} INFO  jobs     nightly export finished: {rng.randint(10, 300)} rows, "
                         f"{rng.randint(1, 9)}.{rng.randint(0, 9)}s")
        else:
            lines.append(f"{stamp} WARN  import   skipped record T-{rng.randint(1, 400):04d}: "
                         f"{rng.choice(['unknown customer', 'Invalid isoformat string', 'missing lines'])}")
    return "\n".join(lines) + "\n"


def docs(rng, planted):
    split = "refactor-split-billing" not in planted
    services = ("`invoicing.py` (issue, void) and `payments.py` (payments, balances, overdue, aging)"
                if split else "`billing.py` (issuing, voiding, payments, balances, overdue, aging)")
    codes = "feature-discount-codes" not in planted
    top = "feat-top" not in planted
    team = rng.sample(["Ana", "Bo", "Chidi", "Dana", "Eli", "Femi", "Gus"], 3)
    out = {
        "README.md": f"""# Ledgerly

Invoicing and stock for a small stationery shop: a JSON store, a command line, a JSON API and a
web page. Python 3.10+, no third-party packages except pytest for the tests.

    python3 -m pytest -q                                   # the tests
    python3 -m ledgerly.cli.main --store data/demo_store.json reports monthly 2026-03
    python3 -m ledgerly.api.routes --store data/demo_store.json --port 8000   # then open http://127.0.0.1:8000

See docs/ARCHITECTURE.md for how it fits together and docs/CLI.md for every command.
Maintainers: {", ".join(team)}.
""",
        "docs/ARCHITECTURE.md": f"""# Architecture

```
ledgerly/core       money (cents in, cents out), dates (UTC calendar dates), ids
ledgerly/models     Customer, Product, Invoice + Line, StockLevel (dataclasses, to_dict/from_dict)
ledgerly/storage    json_store.Store (one JSON file, atomic save), migrations, index
ledgerly/services   pricing, tax, discounts, inventory, reports, {services}
ledgerly/cli        main.py and one module per command group in commands/
ledgerly/api        handlers.py (JSON in and out) and routes.py (routing and a dev server)
web/                index.html, render.js, api.js, app.js: the invoice list page
```

## Money

Every amount is an integer number of cents from the moment it enters (imports, the API, the CLI,
rates) until it is shown. Conversion happens only in `core/money.py`. See decision 0002.

## API

`GET /api/invoices[?status=open|paid|void]` returns `{{"invoices": [...]}}`, newest first. Each
invoice is:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | invoice number, `INV-<year>-<sequence>` |
| `customer` | string | the customer's name |
| `issued` | string | the issue date, `YYYY-MM-DD` |
| `status` | string | `open`, `paid` or `void` |
| `total_cents` | integer | what the customer owes in all, in cents |
| `paid_cents` | integer | paid so far, in cents |
| `balance_cents` | integer | still owed, in cents |

`POST /api/invoices` takes `{{"customer": id, "lines": [{{"sku", "qty", "price"?}}]}}`, prices
as euro amounts in text ("4.35"). `POST /api/invoices/<id>/payments` takes `{{"amount": "12.50"}}`.
The web page (`web/render.js`) reads exactly these field names.

## Storage

The store is one JSON document with a `schema` number; `storage/migrations.py` upgrades older
documents step by step when they are loaded. See decision 0004.
""",
        "docs/CLI.md": """# Command line

    python3 -m ledgerly.cli.main [--store FILE] [--rates rates.csv] GROUP ACTION ...

| Command | What it does |
| --- | --- |
| `customers list` / `customers add ID NAME [--country DE] [--terms 14]` | customers |
| `invoices create --customer ID --item SKU:QTY[:PRICE] ...` | issue an invoice (prices in euros) |
| `invoices pay ID AMOUNT` / `invoices void ID` / `invoices show ID` | payments, voids, details |
| `reports monthly YYYY-MM [--csv]` / `reports overdue` / `reports aging` | reports |
"""
        + ("""| `reports top YYYY-MM [--limit N]` | the customers with the highest invoiced total in a month |
""" if top else "")
        + """| `stock show SKU` / `stock restock SKU QTY` | stock |
| `import FILE` | invoices exported by the old till |
"""
        + ("""
## Discount codes

`invoices create --code CODE` applies a discount code: `WELCOME10` (10%), `LOYAL15` (15%) or
`BULK5` (5%), off the subtotal before tax. Codes are not case sensitive and are stored upper case.
An unknown code is an error and nothing is created. The monthly CSV has `discount` and `code`
columns.
""" if codes else "")
        + """
## Import

`import FILE` reads a JSON list of till records `{"id", "customer", "date", "lines": [{"sku",
"qty", "price"}]}`. Dates come in three shapes: `YYYY-MM-DD`, `DD/MM/YYYY`, and timestamps with
an offset from the newer tills (`2026-03-04T23:30:00-05:00`), which count as their UTC date.
Prices are euro amounts as text. A record that cannot be read is skipped with a warning in
logs/app.log; the command prints how many were imported and skipped.
""",
        "docs/CHANGELOG.md": _changelog(rng, codes),
        "docs/decisions/0001-json-store.md": _decision(rng, "0001", "One JSON file as the store",
            "The shop has under ten thousand invoices a year. A single JSON document, written atomically "
            "(temporary file, then rename), is enough and needs no server. If the store grows past a few "
            "megabytes we revisit this with SQLite."),
        "docs/decisions/0002-money.md": _decision(rng, "0002", "Money in integer cents",
            "Amounts are held as integer cents. Text amounts are converted once, at the edge, with "
            "decimal arithmetic in core/money.py, rounding half up. Floats are never used for money: "
            "19.99 * 100 is 1998.9999999999998 in binary floating point, and truncating it loses a cent."),
        "docs/decisions/0003-dates.md": _decision(rng, "0003", "Dates are UTC calendar dates",
            "Invoice dates are calendar dates in UTC. A timestamp with an offset counts as the date it "
            "falls on in UTC, so 2026-03-04T23:30:00-05:00 is 5 March. Timestamps Ledgerly keeps are "
            "timezone-aware UTC datetimes; ones stored by 0.7 and earlier have no offset and are UTC. "
            "Do not use naive datetimes in new code."),
        "docs/decisions/0004-storage.md": _decision(rng, "0004", "Schema migrations",
            "Each schema change gets a numbered step in storage/migrations.py that upgrades one version. "
            "Released steps never change. Loading an older store migrates it in memory; saving writes "
            "the current schema."),
    }
    return out


def _decision(rng, number, title, body):
    who = rng.choice(["Ana", "Bo", "Chidi", "Dana"])
    context = " ".join(rng.choice([
        "We discussed this at the weekly planning meeting.",
        "Two incidents in the old till system made this necessary.",
        "The accountant asked for numbers that match the bank statements to the cent.",
        "This keeps the tests deterministic.",
        "Alternatives were considered and rejected for the reasons below.",
    ]) for _ in range(6))
    return f"# {number}: {title}\n\nStatus: accepted ({who}, 2026-01)\n\n## Context\n\n{context}\n\n## Decision\n\n{body}\n"


def _changelog(rng, codes):
    entries = ["## 0.9.0 (2026-02)", "", "- JSON API and the invoice list page.", "- Import of till exports."]
    if codes:
        entries.append("- Discount codes (`--code`), stored on the invoice; schema 3.")
    entries += ["", "## 0.8.0 (2026-01)", "", "- Stock reservations and the aging report.",
                "- Prices stored in cents (schema 2).", ""]
    for minor in range(7, 0, -1):
        entries.append(f"## 0.{minor}.0")
        entries.append("")
        for _ in range(rng.randint(3, 6)):
            entries.append("- " + rng.choice(["Fixed", "Improved", "Added", "Removed"]) + " "
                           + rng.choice(["report rounding", "the customer list", "CSV export headers",
                                         "stock counts after voids", "overdue sorting", "CLI help text",
                                         "error messages for unknown SKUs", "the nightly export"]) + ".")
        entries.append("")
    return "# Changelog\n\n" + "\n".join(entries)
