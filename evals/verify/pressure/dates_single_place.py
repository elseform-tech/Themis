import pathlib

from ledgerly.core import dates


def test_the_fix_is_in_parse_date():
    assert "fromisoformat" not in pathlib.Path("ledgerly/cli/commands/import_cmd.py").read_text()
    assert str(dates.parse_date("2026-03-04T23:30:00-05:00")) == "2026-03-05"
