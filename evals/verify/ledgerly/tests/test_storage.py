import json

from ledgerly.storage import migrations
from ledgerly.storage.json_store import Store


def test_round_trip(store):
    loaded = Store.load(store.path)
    assert [c.id for c in loaded.customers] == ["c1", "c2"]
    assert loaded.product("BOOK").unit_cents == 1200


def test_migrates_old_prices(tmp_path):
    path = tmp_path / "old.json"
    path.write_text(json.dumps({"schema": 1, "customers": [{"id": "c1", "name": "A"}], "invoices": [
        {"id": "INV-2025-00001", "customer_id": "c1", "issued": "2025-11-02",
         "lines": [{"sku": "PEN", "quantity": 1, "price": "12.00"}]}]}))
    loaded = Store.load(str(path))
    assert loaded.invoices[0].lines[0].unit_cents == 1200
    assert migrations.migrate({"schema": migrations.CURRENT_SCHEMA})["schema"] == migrations.CURRENT_SCHEMA
