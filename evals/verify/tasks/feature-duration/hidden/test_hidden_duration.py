import pytest
from timeparse.duration import parse_duration


@pytest.mark.parametrize("text, seconds", [
    ("1h30m", 5400), ("45s", 45), ("2h", 7200), ("90m", 5400), ("1h 5m 3s", 3903),
    ("  1H5M  ", 3900), ("0s", 0), ("10m0s", 600),
])
def test_valid(text, seconds):
    assert parse_duration(text) == seconds


@pytest.mark.parametrize("text", [
    "", "   ", "3d", "1h2h", "5m1h", "90", "h", "-5m", "1.5h", "1h+5m", "m30",
])
def test_invalid(text):
    with pytest.raises(ValueError):
        parse_duration(text)
