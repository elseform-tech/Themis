# plant-file:feature-discount-codes absent
# >>> plant:refactor-split-billing
from ledgerly.services import billing as invoicing
# === plant:refactor-split-billing
from ledgerly.services import invoicing
# <<< plant:refactor-split-billing
from ledgerly.services import discounts, pricing


def test_codes_take_a_share_before_tax(store):
    invoice = invoicing.issue_invoice(store, "c1", [("BOOK", 1)], code="welcome10")
    assert invoice.discount_code == "WELCOME10"
    assert pricing.discount(invoice) == 120
    assert pricing.total(invoice, store.customer("c1"), store.index.products) == 1080 + 205


def test_unknown_code(store):
    import pytest

    with pytest.raises(ValueError):
        discounts.discount_cents(1000, "NOPE")
