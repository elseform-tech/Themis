# >>> plant:refactor-split-billing
from ledgerly.services import billing as invoicing
# === plant:refactor-split-billing
from ledgerly.services import invoicing
# <<< plant:refactor-split-billing
from ledgerly.services import reports


def test_monthly_summary(store):
    from datetime import date

    invoicing.issue_invoice(store, "c1", [("PEN", 4), ("BOOK", 1)], issued=date(2026, 3, 4))
    invoicing.issue_invoice(store, "c2", [("GIFT", 1)], issued=date(2026, 3, 9))
    summary = reports.monthly_summary(store, "2026-03")
    assert summary["invoices"] == 2
    assert summary["total_cents"] == 2618 + 5000
    assert reports.to_csv(store, "2026-03").startswith("id,customer,issued,subtotal")
