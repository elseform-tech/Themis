# >>> plant:refactor-split-billing
from ledgerly.services import billing as invoicing
# === plant:refactor-split-billing
from ledgerly.services import invoicing
# <<< plant:refactor-split-billing


def test_issue_reserves_stock(store):
    invoice = invoicing.issue_invoice(store, "c1", [("PEN", 3), ("BOOK", 1, 1000)])
    assert invoice.id.startswith("INV-") and invoice.status == "open"
    assert invoice.lines[1].unit_cents == 1000
    assert store.stock_of("PEN").reserved == 3


def test_void_releases_stock(store):
    invoice = invoicing.issue_invoice(store, "c1", [("PEN", 3)])
    invoicing.void_invoice(store, invoice.id)
    assert store.invoice(invoice.id).status == "void"
    assert store.stock_of("PEN").reserved == 0
