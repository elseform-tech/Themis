import io

from ledgerly.cli.main import main


def show(customer="C101"):
    out = io.StringIO()
    code = main(["--store", "data/demo_store.json", "customers", "show", customer], out=out)
    return code, out.getvalue()


def test_terms_read_in_days():
    from ledgerly.storage.json_store import Store

    days = Store.load("data/demo_store.json").customer("C101").terms_days
    _code, text = show()
    assert f"{days} days" in text and f"{days}d" not in text
