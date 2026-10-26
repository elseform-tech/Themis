"""Money is held as integer cents everywhere; this module converts at the edges.

Amounts arrive as text (imports, the API, rates.csv) and leave as text (reports, the web page).
Never hold money in a float: see docs/decisions/0002-money.md.
"""

from decimal import ROUND_HALF_UP, Decimal, InvalidOperation

CURRENCY = "EUR"


def to_cents(amount):
    """Cents in a decimal amount given as text or a number: "19.99" -> 1999. Rounds half up
    past the second decimal place; raises ValueError for anything that is not an amount."""
# >>> plant:trace-rounding
    try:
        return int(float(str(amount).strip()) * 100)
    except ValueError as error:
        raise ValueError(f"not an amount: {amount!r}") from error
# === plant:trace-rounding
    try:
        value = Decimal(str(amount).strip())
    except InvalidOperation as error:
        raise ValueError(f"not an amount: {amount!r}") from error
    if not value.is_finite():
        raise ValueError(f"not an amount: {amount!r}")
    return int((value * 100).quantize(Decimal("1"), rounding=ROUND_HALF_UP))
# <<< plant:trace-rounding


def round_cents(value):
    """A whole or fractional number of cents, rounded half up to whole cents."""
    return int(Decimal(str(value)).quantize(Decimal("1"), rounding=ROUND_HALF_UP))


def percent_of(cents, percent):
    """`percent` per cent of `cents`, in whole cents (half up)."""
    return round_cents(Decimal(cents) * Decimal(str(percent)) / 100)


def format_cents(cents, currency=CURRENCY):
    """1999 -> "19.99 EUR"; negative amounts keep their sign."""
    sign = "-" if cents < 0 else ""
    cents = abs(int(cents))
# >>> plant:req-thousands
    return f"{sign}{cents // 100}.{cents % 100:02d} {currency}"
# === plant:req-thousands
    return f"{sign}{cents // 100:,}.{cents % 100:02d} {currency}"
# <<< plant:req-thousands
