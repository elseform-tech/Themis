# >>> plant:refactor-split-billing
from ledgerly.services import billing as invoicing
# === plant:refactor-split-billing
from ledgerly.services import invoicing
# <<< plant:refactor-split-billing
# >>> plant:refactor-split-billing
from ledgerly.services import billing as payments
# === plant:refactor-split-billing
from ledgerly.services import payments
# <<< plant:refactor-split-billing

from datetime import date, datetime, timezone


def test_pay_in_full_closes_and_ships(store):
    invoice = invoicing.issue_invoice(store, "c1", [("PEN", 2)])
    owed = payments.balance(store, invoice)
    payments.record_payment(store, invoice.id, owed - 100)
    assert store.invoice(invoice.id).status == "open"
    payments.record_payment(store, invoice.id, 100)
    assert store.invoice(invoice.id).status == "paid"
    assert store.stock_of("PEN").on_hand == 98


def test_overdue(store):
    invoice = invoicing.issue_invoice(store, "c1", [("PEN", 1)], issued=date(2026, 1, 2))
    now = datetime(2026, 2, 1, tzinfo=timezone.utc)
    assert [late.id for late in payments.overdue(store, now)] == [invoice.id]
