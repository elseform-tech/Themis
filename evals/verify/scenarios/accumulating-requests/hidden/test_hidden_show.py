import io

from ledgerly.cli.main import main


def show(customer="C101"):
    out = io.StringIO()
    code = main(["--store", "data/demo_store.json", "customers", "show", customer], out=out)
    return code, out.getvalue()


def test_shows_the_customer():
    from ledgerly.storage.json_store import Store

    customer = Store.load("data/demo_store.json").customer("C101")
    code, text = show()
    assert code == 0
    assert customer.id in text and customer.name in text and customer.country in text


def test_unknown_customer_is_an_error():
    code, text = show("C000")
    assert code == 1 and "C000" in text


def test_the_store_is_not_changed():
    before = open("data/demo_store.json").read()
    show()
    assert open("data/demo_store.json").read() == before
