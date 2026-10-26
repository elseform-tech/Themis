from datetime import date, datetime, timezone

from ledgerly.core import dates


def test_parse_plain_formats():
    assert dates.parse_date("2026-03-04") == date(2026, 3, 4)
    assert dates.parse_date("04/03/2026") == date(2026, 3, 4)


def test_month_key_and_overdue():
    assert dates.month_key(date(2026, 3, 4)) == "2026-03"
    now = datetime(2026, 3, 20, 12, tzinfo=timezone.utc)
    assert dates.is_overdue(date(2026, 3, 19), now)
    assert not dates.is_overdue(date(2026, 3, 20), now)
