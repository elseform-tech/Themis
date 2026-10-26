import io
import json

from ledgerly.api import handlers
from ledgerly.cli.commands import import_cmd, invoices
from ledgerly.cli.main import main
from ledgerly.core import money
from ledgerly.storage.json_store import Store

TRICKY = {"19.99": 1999, "4.35": 435, "0.29": 29, "1.15": 115, "2.85": 285, "3.29": 329, "8.95": 895, "0.57": 57}


def test_money_reads_every_tricky_amount():
    assert {text: money.to_cents(text) for text in TRICKY} == TRICKY


def test_import_keeps_the_cent():
    invoice = import_cmd.record_to_invoice(
        {"id": "T-9", "customer": "C101", "date": "2026-03-02", "lines": [{"sku": "INK", "qty": 1, "price": "19.99"}]}, {})
    assert invoice.lines[0].unit_cents == 1999


def test_api_keeps_the_cent(tmp_path):
    path = tmp_path / "s.json"
    path.write_text(open("data/demo_store.json").read())
    store = Store.load(str(path))
    view = handlers.create_invoice(store, {"customer": "C101", "lines": [{"sku": "TAPE", "qty": 1, "price": "4.35"}]})
    assert store.invoice(view["id"]).lines[0].unit_cents == 435


def test_cli_items_and_payments_keep_the_cent():
    assert invoices.parse_item("CLIP:3:0.29") == ("CLIP", 3, 29)
    assert money.to_cents("8.95") == 895


def test_still_rejects_text_and_rounds_half_up():
    import pytest

    with pytest.raises(ValueError):
        money.to_cents("ten")
    assert money.to_cents("0.005") == 1
    assert money.to_cents("2.675") == 268


def test_monthly_report_after_import(tmp_path):
    store = tmp_path / "s.json"
    store.write_text(open("data/demo_store.json").read())
    till = tmp_path / "till.json"
    till.write_text(json.dumps([{"id": "T-1", "customer": "C101", "date": "2026-04-02",
                                 "lines": [{"sku": "INK", "qty": 3, "price": "19.99"}]}]))
    out = io.StringIO()
    assert main(["--store", str(store), "import", str(till)], out=out) == 0
    out = io.StringIO()
    main(["--store", str(store), "reports", "monthly", "2026-04", "--csv"], out=out)
    assert ",5997," in out.getvalue()
