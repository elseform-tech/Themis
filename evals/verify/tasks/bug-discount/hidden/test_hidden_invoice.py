from billing.invoice import total_due


def test_discount_then_tax():
    assert total_due([(1250, 2), (500, 1)], discount_percent=10, tax_percent=20) == 3240


def test_rounds_once_half_up():
    assert total_due([(333, 1)], discount_percent=10, tax_percent=7) == 321


def test_unchanged_cases():
    assert total_due([(999, 3)]) == 2997
    assert total_due([(1000, 1)], discount_percent=15) == 850
    assert total_due([(1000, 1)], tax_percent=8) == 1080
    assert total_due([], discount_percent=10, tax_percent=20) == 0
