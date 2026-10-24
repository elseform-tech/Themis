"""Reference settlement engine: the rules of docs/overview.md, driven by one desk's RULES.

The generator imports this file to compute the hidden tests' expected values, and the reference
solution ships a copy of it as `ledger/engine.py`. Standard library only.
"""

import datetime

ORDERS = {
    "amount_desc": lambda e: (-e["amount"], e["id"]),
    "amount_asc": lambda e: (e["amount"], e["id"]),
    "account": lambda e: (e["account"], e["id"]),
    "date": lambda e: (e["date"], e["id"]),
}


def round_fee(numerator, mode):
    """Integer cents of numerator / 10000 under a rounding mode (amounts are positive)."""
    whole, rest = divmod(numerator, 10000)
    if mode == "down":
        return whole
    if mode == "up":
        return whole + (rest > 0)
    if mode == "half_up":
        return whole + (rest * 2 >= 10000)
    if mode == "half_even":
        return whole + (rest * 2 > 10000 or (rest * 2 == 10000 and whole % 2 == 1))
    raise ValueError(f"unknown rounding {mode}")


def ordered(rules, entries):
    if rules["order"] == "id_desc":
        return sorted(entries, key=lambda e: e["id"], reverse=True)
    return sorted(entries, key=ORDERS[rules["order"]])


def rejection(rules, entry):
    """The error code that rejects an entry, or None when the desk accepts it."""
    if entry["currency"] != rules["currency"]:
        return rules["code"] + "-" + str(rules["currency_error"])
    if entry["amount"] < rules["floor"]:
        return rules["code"] + "-" + str(rules["floor_error"])
    return None


def fee(rules, amount):
    return max(round_fee(amount * rules["bps"], rules["rounding"]), rules["min_fee"])


def settle(rules, entries):
    lines, errors = [], []
    for entry in entries:
        code = rejection(rules, entry)
        if code:
            errors.append({"id": entry["id"], "code": code})
    accepted = [e for e in entries if rejection(rules, e) is None]
    for entry in ordered(rules, accepted):
        charged = fee(rules, entry["amount"])
        lines.append({"id": entry["id"], "account": entry["account"], "amount": entry["amount"],
                      "fee": charged, "net": entry["amount"] - charged})
    return {"lines": lines, "errors": sorted(errors, key=lambda e: e["id"])}


def release_date(rules, date):
    day = datetime.date.fromisoformat(date)
    left = rules["hold_days"]
    while left > 0:
        day += datetime.timedelta(days=1)
        if rules["calendar"] == "calendar" or day.weekday() < 5:
            left -= 1
    return day.isoformat()


def holds(rules, entries, today):
    accepted = [e for e in entries if rejection(rules, e) is None]
    released, held = [], []
    for entry in accepted:
        until = release_date(rules, entry["date"])
        if until <= today:
            released.append(entry)
        else:
            held.append({"id": entry["id"], "until": until})
    return {"released": [e["id"] for e in ordered(rules, released)],
            "held": sorted(held, key=lambda h: (h["until"], h["id"]))}
