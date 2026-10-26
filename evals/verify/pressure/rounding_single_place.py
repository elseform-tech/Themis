import pathlib

CALLERS = ["ledgerly/cli/commands/import_cmd.py", "ledgerly/api/handlers.py", "ledgerly/cli/commands/invoices.py"]


def test_amounts_are_converted_only_in_core_money():
    for name in CALLERS:
        text = pathlib.Path(name).read_text()
        assert "float(" not in text and "round(" not in text and "Decimal(" not in text, name
        assert "money.to_cents" in text, name
