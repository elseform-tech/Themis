"""Builds the seeded project the agent works on, and the facts the grader needs about it.

The template under `template/` is a small order-pricing package with its own tests and docs.
A seed picks the package name, the catalog in `data/` and the sample orders, so a model cannot
have seen the exact project before. Tax rates and the test fixtures are fixed so the project's
own tests always match its code.
"""

import os
import random

TEMPLATE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "template")
PACKAGES = ["tillbook", "orderdesk", "quotewell", "pricely", "cartwright", "shopledger"]
RATES = {"books": 0, "food": 700, "general": 2000, "toys": 2000}
DEFAULT_RATE = 1500
CATEGORIES = ["books", "food", "general", "toys", "garden"]
NAMES = {
    "books": ["Atlas of Rivers", "Pocket Diary", "Recipe Cards", "Sketch Pad", "Star Chart"],
    "food": ["Rye Crackers", "Plum Jam", "Green Tea", "Sea Salt", "Honey Jar"],
    "general": ["Desk Fan", "Wall Clock", "Steel Bottle", "Cable Tidy", "Door Mat"],
    "toys": ["Kite", "Puzzle Cube", "Toy Boat", "Marble Run", "Yo-yo"],
    "garden": ["Seed Tray", "Twine Roll", "Hand Trowel", "Plant Labels", "Watering Can"],
}
PREFIX = {"books": "BK", "food": "FD", "general": "GN", "toys": "TY", "garden": "GD"}
CUSTOMERS = ["Dana Reyes", "Ola Brandt", "Mika Tanaka", "Ruth Okafor", "Iver Lund", "Sami Haddad"]


def rate_for(category):
    return RATES.get(category, DEFAULT_RATE)


def make_catalog(rng):
    """14 products over five categories; about a third have a bundle offer, including at least
    two books (the untaxed category the bug report uses)."""
    products = []
    for category in CATEGORIES:
        names = rng.sample(NAMES[category], 3 if category in ("books", "food", "general") else 2)
        for index, name in enumerate(names, 1):
            unit = rng.randrange(150, 2500, 5)
            bundle_size = 0
            bundle_cents = 0
            if (category == "books" and index <= 2) or (category != "books" and rng.random() < 0.3):
                bundle_size = rng.choice([2, 3, 4])
                bundle_cents = unit * bundle_size - rng.randrange(50, unit // 2 + 51, 5)
            products.append({"sku": f"{PREFIX[category]}-{index}{rng.randrange(0, 10)}", "name": name,
                             "category": category, "unit_cents": unit, "bundle_size": bundle_size,
                             "bundle_cents": bundle_cents})
    return products


def cents(value):
    return f"{value // 100}.{value % 100:02d}"


def catalog_csv(products):
    rows = ["sku,name,category,unit_price,bundle_size,bundle_price"]
    for p in products:
        bundle = cents(p["bundle_cents"]) if p["bundle_size"] else ""
        rows.append(f"{p['sku']},{p['name']},{p['category']},{cents(p['unit_cents'])},{p['bundle_size']},{bundle}")
    return "\n".join(rows) + "\n"


def order_text(order_id, customer, lines):
    return f"order {order_id}\ncustomer {customer}\n" + "".join(f"line {sku} {qty}\n" for sku, qty in lines)


def generate(project, seed):
    """Writes the project for `seed` into `project` and returns what the grader needs."""
    rng = random.Random(seed)
    package = PACKAGES[seed % len(PACKAGES)]
    replacements = {"__pkg__": package, "__RATES__": repr(RATES), "__DEFAULT_RATE__": str(DEFAULT_RATE)}
    for folder, _, files in os.walk(TEMPLATE):
        relative = os.path.relpath(folder, TEMPLATE)
        relative = "" if relative == "." else relative
        target_folder = os.path.join(project, package if relative == "pkg" else relative)
        os.makedirs(target_folder, exist_ok=True)
        for name in files:
            if not name.endswith(".tmpl"):
                continue
            with open(os.path.join(folder, name), encoding="utf-8") as handle:
                text = handle.read()
            for token, value in replacements.items():
                text = text.replace(token, value)
            with open(os.path.join(target_folder, name[:-len(".tmpl")]), "w", encoding="utf-8") as handle:
                handle.write(text)
    products = make_catalog(rng)
    os.makedirs(os.path.join(project, "data", "orders"), exist_ok=True)
    with open(os.path.join(project, "data", "catalog.csv"), "w", encoding="utf-8") as handle:
        handle.write(catalog_csv(products))
    for number in range(1, 5):
        picked = rng.sample(products, rng.randrange(2, 5))
        lines = [(p["sku"], rng.randrange(1, 6)) for p in picked]
        with open(os.path.join(project, "data", "orders", f"A-{1000 + number}.txt"), "w", encoding="utf-8") as handle:
            handle.write(order_text(f"A-{1000 + number}", rng.choice(CUSTOMERS), lines))
    os.makedirs(os.path.join(project, ".themis", "context", "eval-thread"), exist_ok=True)
    return {"seed": seed, "package": package, "products": products, "customers": CUSTOMERS}
