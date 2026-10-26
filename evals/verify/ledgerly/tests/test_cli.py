import io

from ledgerly.cli.main import main


def run(store, *args):
    out = io.StringIO()
    code = main(["--store", store.path, *args], out=out)
    return code, out.getvalue()


def test_create_and_show(store):
    code, text = run(store, "invoices", "create", "--customer", "c1", "--item", "PEN:2")
    assert code == 0 and text.startswith("INV-")
    code, text = run(store, "invoices", "show", text.split()[0])
    assert "total 5.95 EUR" in text


def test_unknown_customer_is_an_error(store):
    code, text = run(store, "invoices", "create", "--customer", "nobody", "--item", "PEN:1")
    assert code == 1 and "unknown customer" in text
