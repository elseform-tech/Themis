import pathlib
import subprocess
import sys
from datetime import datetime, timezone

from ledgerly.core import dates
from ledgerly.services import payments
from ledgerly.storage.json_store import Store


def test_tests_pass_with_deprecations_as_errors():
    done = subprocess.run([sys.executable, "-W", "error::DeprecationWarning", "-m", "pytest", "-q", "-p",
                           "no:cacheprovider", "-W", "error::DeprecationWarning", "tests"], capture_output=True, text=True)
    assert done.returncode == 0, done.stdout[-600:]


def test_no_utcnow_left():
    assert not [str(p) for p in pathlib.Path("ledgerly").rglob("*.py") if "utcnow(" in p.read_text()]


def test_now_is_aware_utc():
    now = dates.utc_now()
    assert now.tzinfo is not None and now.utcoffset().total_seconds() == 0


def test_aging_reads_old_offsetless_timestamps():
    store = Store.load("data/demo_store.json")
    buckets = payments.aging(store, datetime(2026, 6, 1, tzinfo=timezone.utc))
    assert sum(len(ids) for ids in buckets.values()) > 0
    assert payments.aging(store) is not None


def test_ids_still_stamp():
    from ledgerly.core import ids

    assert len(ids.stamp()) == 15
