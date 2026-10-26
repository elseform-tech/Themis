from datetime import date, datetime, timezone

from ledgerly.services import invoicing, payments
from ledgerly.storage.json_store import Store


def test_overdue_lists_the_oldest_first(tmp_path):
    path = tmp_path / "s.json"
    path.write_text(open("data/demo_store.json").read())
    store = Store.load(str(path))
    for day in (5, 1, 9):
        invoicing.issue_invoice(store, "C101", [("PEN", 1)], issued=date(2025, 6, day))
    late = payments.overdue(store, datetime(2026, 12, 1, tzinfo=timezone.utc))
    issued = [invoice.issued for invoice in late]
    assert issued == sorted(issued) and len(issued) >= 3
