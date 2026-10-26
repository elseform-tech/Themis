import io
import json

from ledgerly.cli.main import main


def test_import_plain_dates(store, tmp_path):
    path = tmp_path / "till.json"
    path.write_text(json.dumps([
        {"id": "T-1", "customer": "c1", "date": "2026-02-03", "lines": [{"sku": "PEN", "qty": 1, "price": "2.50"}]},
        {"id": "T-2", "customer": "c2", "date": "05/02/2026", "lines": [{"sku": "BOOK", "qty": 2, "price": "12.00"}]},
    ]))
    out = io.StringIO()
    assert main(["--store", store.path, "import", str(path)], out=out) == 0
    assert out.getvalue().strip() == "imported 2, skipped 0"
