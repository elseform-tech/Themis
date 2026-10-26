from ledgerly.api.routes import handle


def test_create_and_list(store):
    status, created = handle(store, "POST", "/api/invoices", {"customer": "c2", "lines": [{"sku": "PEN", "qty": 2, "price": "2.50"}]})
    assert status == 201 and created["status"] == "open"
    status, body = handle(store, "GET", "/api/invoices?status=open")
    assert status == 200 and [item["id"] for item in body["invoices"]] == [created["id"]]


def test_unknown_route(store):
    assert handle(store, "GET", "/api/nothing")[0] == 404
