import io
import json

from ledgerly.cli.main import main


def test_import_names_each_skipped_record(tmp_path):
    store = tmp_path / "s.json"
    store.write_text(open("data/demo_store.json").read())
    till = tmp_path / "till.json"
    till.write_text(json.dumps([
        {"id": "T-OK", "customer": "C101", "date": "2026-03-02", "lines": [{"sku": "PEN", "qty": 1, "price": "2.50"}]},
        {"id": "T-BAD", "customer": "nobody", "date": "2026-03-02", "lines": [{"sku": "PEN", "qty": 1, "price": "2.50"}]},
    ]))
    out = io.StringIO()
    main(["--store", str(store), "import", str(till)], out=out)
    lines = out.getvalue().strip().splitlines()
    assert any(line.startswith("skipped T-BAD") for line in lines), lines
    assert lines[-1] == "imported 1, skipped 1"
