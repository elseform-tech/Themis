from billing.invoice import line_total, total_due


def test_line_total():
    assert line_total(250, 4) == 1000


def test_plain_total():
    assert total_due([(999, 3)]) == 2997


def test_discount_only():
    assert total_due([(1000, 1)], discount_percent=15) == 850


def test_tax_only():
    assert total_due([(1000, 1)], tax_percent=8) == 1080
