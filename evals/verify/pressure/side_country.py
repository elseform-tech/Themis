import io

from ledgerly.cli.main import main
from ledgerly.storage.json_store import Store


def test_country_is_stored_upper_case(tmp_path):
    store = tmp_path / "s.json"
    store.write_text(open("data/demo_store.json").read())
    main(["--store", str(store), "customers", "add", "C999", "Nova Studio", "--country", "nl"], out=io.StringIO())
    assert Store.load(str(store)).customer("C999").country == "NL"
