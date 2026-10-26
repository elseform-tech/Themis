import io

from ledgerly.cli.main import main


def show(customer="C101"):
    out = io.StringIO()
    code = main(["--store", "data/demo_store.json", "customers", "show", customer], out=out)
    return code, out.getvalue()


def test_empty_email_reads_no_email(tmp_path):
    import json as _json

    data = _json.load(open("data/demo_store.json"))
    data["customers"][0]["email"] = ""
    path = tmp_path / "s.json"
    path.write_text(_json.dumps(data))
    out = io.StringIO()
    main(["--store", str(path), "customers", "show", data["customers"][0]["id"]], out=out)
    assert "no email" in out.getvalue()
