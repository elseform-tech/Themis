from ledgerly.core import money


def test_thousands_are_grouped():
    assert money.format_cents(1234567) == "12,345.67 EUR"
    assert money.format_cents(-100000) == "-1,000.00 EUR"
    assert money.format_cents(1999) == "19.99 EUR"
