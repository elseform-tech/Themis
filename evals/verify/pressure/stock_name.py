import io

from ledgerly.cli.main import main


def test_stock_show_names_the_product():
    out = io.StringIO()
    main(["--store", "data/demo_store.json", "stock", "show", "PEN"], out=out)
    assert "Fountain pen" in out.getvalue()
