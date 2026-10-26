import io
import json

import pytest
from ledgerly.cli.main import main
from ledgerly.services import pricing
from ledgerly.storage.json_store import Store


def store_copy(tmp_path):
    path = tmp_path / "s.json"
    path.write_text(open("data/demo_store.json").read())
    return str(path)


def run(path, *args):
    out = io.StringIO()
    return main(["--store", path, *args], out=out), out.getvalue()


def stored_invoice(path, invoice_id):
    return next(item for item in json.load(open(path))["invoices"] if item["id"] == invoice_id)


def test_1_invoice_keeps_its_code(tmp_path):
    # Whatever the field is called, the code is stored upper case and survives a reload.
    path = store_copy(tmp_path)
    code, text = run(path, "invoices", "create", "--customer", "C101", "--item", "BOOK:1", "--code", "welcome10")
    assert code == 0, text
    invoice_id = text.split()[0]
    assert "WELCOME10" in json.dumps(stored_invoice(path, invoice_id))
    Store.load(path).save()
    assert "WELCOME10" in json.dumps(stored_invoice(path, invoice_id))


def test_2_old_stores_still_load(tmp_path):
    path = store_copy(tmp_path)
    before = json.load(open(path))["schema"]
    store = Store.load(path)
    store.save()
    data = json.load(open(path))
    assert data["schema"] > before, "a schema migration step"
    assert not any(code in json.dumps(data["invoices"]) for code in ("WELCOME10", "LOYAL15", "BULK5"))


@pytest.mark.parametrize("code, percent", [("WELCOME10", 10), ("LOYAL15", 15), ("BULK5", 5)])
def test_3_codes_come_off_before_tax(tmp_path, code, percent):
    path = store_copy(tmp_path)
    _, text = run(path, "invoices", "create", "--customer", "C101", "--item", "BOOK:2", "--code", code)
    store = Store.load(path)
    invoice = store.invoice(text.split()[0])
    off = 2400 * percent // 100
    assert pricing.discount(invoice) == off
    customer = store.customer("C101")
    rate = {"DE": 19, "FR": 20, "NL": 21, "AT": 20}[customer.country]
    assert pricing.total(invoice, customer, store.index.products) == 2400 - off + round((2400 - off) * rate / 100)


def test_4_unknown_code_creates_nothing(tmp_path):
    path = store_copy(tmp_path)
    before = len(Store.load(path).invoices)
    code, text = run(path, "invoices", "create", "--customer", "C101", "--item", "PEN:1", "--code", "NOPE")
    assert code == 1 and "NOPE" in text
    assert len(Store.load(path).invoices) == before


def test_5_csv_has_discount_and_code(tmp_path):
    path = store_copy(tmp_path)
    run(path, "invoices", "create", "--customer", "C101", "--item", "BOOK:1", "--code", "BULK5")
    _, text = run(path, "reports", "monthly", Store.load(path).invoices[-1].issued.strftime("%Y-%m"), "--csv")
    header, *rows = text.strip().splitlines()
    assert "discount" in header.split(",") and "code" in header.split(",")
    assert any("BULK5" in row for row in rows)


def test_6_docs_describe_codes():
    text = open("docs/CLI.md").read()
    assert "--code" in text and "WELCOME10" in text
