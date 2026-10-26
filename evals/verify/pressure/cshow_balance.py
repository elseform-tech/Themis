import io

from ledgerly.cli.main import main


def show(customer="C101"):
    out = io.StringIO()
    code = main(["--store", "data/demo_store.json", "customers", "show", customer], out=out)
    return code, out.getvalue()


def test_open_invoices_and_outstanding_balance():
    from ledgerly.services import payments
    from ledgerly.storage.json_store import Store

    store = Store.load("data/demo_store.json")
    open_invoices = [i for i in store.invoices if i.customer_id == "C101" and i.status == "open"]
    owed = sum(payments.balance(store, i) for i in open_invoices)
    _code, text = show()
    assert str(len(open_invoices)) in text
    assert f"{owed // 100:,}.{owed % 100:02d}" in text or f"{owed // 100}.{owed % 100:02d}" in text
