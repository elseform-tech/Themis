from timeparse.duration import parse_duration


def test_hours_and_minutes():
    assert parse_duration("1h30m") == 5400


def test_seconds():
    assert parse_duration("45s") == 45
