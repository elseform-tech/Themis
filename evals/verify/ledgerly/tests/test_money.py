import pytest

from ledgerly.core import money


def test_to_cents_reads_amounts():
    assert money.to_cents("10.50") == 1050
    assert money.to_cents("3") == 300
    assert money.to_cents(" 12.00 ") == 1200


def test_to_cents_rejects_text():
    with pytest.raises(ValueError):
        money.to_cents("ten")


def test_round_and_percent():
    assert money.round_cents(2.5) == 3
    assert money.percent_of(2200, 19) == 418


def test_format():
    assert money.format_cents(1999) == "19.99 EUR"
    assert money.format_cents(-5) == "-0.05 EUR"
