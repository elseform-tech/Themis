from datetime import date

from ledgerly.models.invoice import Invoice, Line
from ledgerly.services import pricing


def test_totals(store):
    invoice = Invoice("INV-2026-00001", "c1", date(2026, 3, 4), [Line("PEN", 4, 250), Line("BOOK", 1, 1200)])
    customer = store.customer("c1")
    assert pricing.subtotal(invoice) == 2200
    assert pricing.tax(invoice, customer, store.index.products) == 418
    assert pricing.total(invoice, customer, store.index.products) == 2618


def test_untaxed_lines(store):
    invoice = Invoice("INV-2026-00002", "c2", date(2026, 3, 4), [Line("GIFT", 1, 5000), Line("PEN", 2, 250)])
    assert pricing.total(invoice, store.customer("c2"), store.index.products) == 5600
