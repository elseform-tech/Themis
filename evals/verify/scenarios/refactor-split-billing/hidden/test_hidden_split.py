import importlib.util
import pathlib
import subprocess
import sys
from datetime import date, datetime, timezone


def test_billing_is_gone():
    assert importlib.util.find_spec("ledgerly.services.billing") is None


def test_no_reference_to_billing_left():
    found = []
    for folder in ("ledgerly", "tests", "docs"):
        for path in pathlib.Path(folder).rglob("*"):
            if path.suffix in (".py", ".md") and ("services.billing" in path.read_text() or "import billing" in path.read_text()):
                found.append(str(path))
    assert found == []


def test_invoicing_issues_and_voids(tmp_path):
    from ledgerly.services import invoicing
    from ledgerly.storage.json_store import Store

    path = tmp_path / "s.json"
    path.write_text(open("data/demo_store.json").read())
    store = Store.load(str(path))
    invoice = invoicing.issue_invoice(store, "C101", [("PEN", 2)], issued=date(2026, 4, 1))
    assert invoicing.void_invoice(store, invoice.id).status == "void"


def test_payments_behave_as_before(tmp_path):
    from ledgerly.services import invoicing, payments
    from ledgerly.storage.json_store import Store

    path = tmp_path / "s.json"
    path.write_text(open("data/demo_store.json").read())
    store = Store.load(str(path))
    invoice = invoicing.issue_invoice(store, "C101", [("BOOK", 1)], issued=date(2026, 1, 1))
    owed = payments.balance(store, invoice)
    payments.record_payment(store, invoice.id, owed)
    assert store.invoice(invoice.id).status == "paid"
    later = datetime(2026, 6, 1, tzinfo=timezone.utc)
    assert all(late.status == "open" for late in payments.overdue(store, later))
    assert set(payments.aging(store, later)) == {"0-30", "31-60", "61+"}
    assert payments.due_date(store, invoice) > invoice.issued


def test_the_project_tests_pass():
    done = subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", "tests"],
                          capture_output=True, text=True)
    assert done.returncode == 0, done.stdout[-500:]


def test_the_cli_still_works(tmp_path):
    path = tmp_path / "s.json"
    path.write_text(open("data/demo_store.json").read())
    done = subprocess.run([sys.executable, "-m", "ledgerly.cli.main", "--store", str(path), "reports", "aging"],
                          capture_output=True, text=True)
    assert done.returncode == 0 and "0-30" in done.stdout
