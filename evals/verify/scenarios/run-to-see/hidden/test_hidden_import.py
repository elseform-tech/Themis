import io
import json
from datetime import date

from ledgerly.cli.main import main
from ledgerly.core import dates
from ledgerly.storage.json_store import Store


def import_sample(tmp_path):
    path = tmp_path / "s.json"
    path.write_text(open("data/demo_store.json").read())
    out = io.StringIO()
    main(["--store", str(path), "import", "data/sample_invoices.json"], out=out)
    return Store.load(str(path)), out.getvalue()


def test_every_sample_record_imports(tmp_path):
    _store, text = import_sample(tmp_path)
    total = len(json.load(open("data/sample_invoices.json")))
    assert text.strip() == f"imported {total}, skipped 0"


def test_offset_timestamps_count_as_their_utc_date():
    assert dates.parse_date("2026-03-04T23:30:00-05:00") == date(2026, 3, 5)
    assert dates.parse_date("2026-03-04T12:00:00+02:00") == date(2026, 3, 4)


def test_imported_dates_match_the_records(tmp_path):
    store, _text = import_sample(tmp_path)
    for record in json.load(open("data/sample_invoices.json")):
        assert store.invoice(record["id"]).issued == dates.parse_date(record["date"])


def test_plain_dates_still_work():
    assert dates.parse_date("04/03/2026") == date(2026, 3, 4)
    assert dates.parse_date("2026-03-04") == date(2026, 3, 4)
