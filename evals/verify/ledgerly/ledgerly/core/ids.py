"""Identifiers: invoice numbers are "INV-<year>-<sequence>", unique per store."""

from ledgerly.core import dates


def next_invoice_id(existing, year=None):
    """The next free invoice number for `year` (this year by default) given existing ids."""
    year = year or dates.today().year
    prefix = f"INV-{year}-"
    used = [int(item[len(prefix):]) for item in existing if item.startswith(prefix)]
    return f"{prefix}{(max(used) + 1 if used else 1):05d}"


def stamp():
    """A sortable timestamp for logs and audit lines."""
    return dates.utc_now().strftime("%Y%m%dT%H%M%S")
