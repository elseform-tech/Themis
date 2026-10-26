import io

from ledgerly.cli.main import main
from ledgerly.services import pricing
from ledgerly.storage.json_store import Store


def run(path, *args):
    out = io.StringIO()
    code = main(["--store", str(path), *args], out=out)
    return code, out.getvalue().strip().splitlines()


def store_copy(tmp_path):
    path = tmp_path / "s.json"
    path.write_text(open("data/demo_store.json").read())
    return path


def expected(path, month):
    store = Store.load(str(path))
    totals = {}
    for invoice in store.invoices:
        if invoice.issued.strftime("%Y-%m") == month:
            customer = store.customer(invoice.customer_id)
            totals[customer.id] = totals.get(customer.id, 0) + pricing.total(invoice, customer, store.index.products)
    return sorted(((cid, cents) for cid, cents in totals.items() if cents > 0), key=lambda p: (-p[1], p[0]))


def test_ranks_by_month_total(tmp_path):
    path = store_copy(tmp_path)
    code, lines = run(path, "reports", "top", "2026-03", "--limit", "3")
    assert code == 0
    want = expected(path, "2026-03")[:3]
    assert [line.split("\t")[1] for line in lines] == [cid for cid, _ in want]
    assert [line.split("\t")[0] for line in lines] == [str(n) for n in range(1, len(want) + 1)]


def test_limit_defaults_to_five(tmp_path):
    path = store_copy(tmp_path)
    _code, lines = run(path, "reports", "top", "2026-02")
    assert len(lines) == min(5, len(expected(path, "2026-02")))


def test_shows_name_and_total(tmp_path):
    path = store_copy(tmp_path)
    _code, lines = run(path, "reports", "top", "2026-01", "--limit", "1")
    cid, cents = expected(path, "2026-01")[0]
    name = Store.load(str(path)).customer(cid).name
    assert name in lines[0] and f"{cents // 100:,}.{cents % 100:02d} EUR" in lines[0]


def test_a_month_without_invoices_prints_nothing(tmp_path):
    path = store_copy(tmp_path)
    code, lines = run(path, "reports", "top", "2025-07")
    assert code == 0 and lines == []


def test_bad_limit_is_an_error(tmp_path):
    path = store_copy(tmp_path)
    code, _lines = run(path, "reports", "top", "2026-03", "--limit", "0")
    assert code == 1


def test_documented():
    assert "reports top" in open("docs/CLI.md").read()
