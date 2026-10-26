"""Discounts."""
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes

from ledgerly.core import money

# Code -> percent off the subtotal before tax (docs/CLI.md, "Discount codes").
CODES = {"WELCOME10": 10, "LOYAL15": 15, "BULK5": 5}


def normalize(code):
    """A code as stored: trimmed and upper case; empty means none."""
    code = (code or "").strip().upper()
    return code or None


def discount_cents(subtotal_cents, code):
    """Cents off for `code` (0 without one); an unknown code is a ValueError."""
    code = normalize(code)
    if code is None:
        return 0
    if code not in CODES:
        raise ValueError(f"unknown discount code {code!r}")
    return money.percent_of(subtotal_cents, CODES[code])
# <<< plant:feature-discount-codes
