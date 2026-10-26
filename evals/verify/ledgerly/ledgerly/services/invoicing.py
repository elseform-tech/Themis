# plant-file:refactor-split-billing absent
"""Issuing and voiding invoices."""

from ledgerly.core import dates, ids
from ledgerly.models.invoice import Invoice, Line
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
from ledgerly.services import discounts
# <<< plant:feature-discount-codes
from ledgerly.services import inventory


def issue_invoice(store, customer_id, items, code=None, issued=None):
    """A new open invoice for `items` [(sku, quantity) or (sku, quantity, unit_cents)], stock
    reserved, saved. A unit price defaults to the product's."""
    store.customer(customer_id)
    lines = []
    for item in items:
        sku, quantity = item[0], int(item[1])
        if quantity <= 0:
            raise ValueError(f"quantity for {sku} must be positive")
        unit = int(item[2]) if len(item) > 2 else store.product(sku).unit_cents
        lines.append(Line(sku, quantity, unit))
    issued = issued or dates.today()
    invoice = Invoice(ids.next_invoice_id(store.index.invoices, issued.year), customer_id, issued, lines,
                      created_at=dates.utc_now())
# >>> plant:feature-discount-codes
    if code:
        raise ValueError("discount codes are not supported")
# === plant:feature-discount-codes
    invoice.discount_code = discounts.normalize(code)
    discounts.discount_cents(0, invoice.discount_code)  # unknown codes fail before anything changes
# <<< plant:feature-discount-codes
    for line in lines:
        inventory.reserve(store, line.sku, line.quantity)
    store.invoices.append(invoice)
    store.save()
    return invoice


def void_invoice(store, invoice_id):
    """Voids an unpaid invoice and releases its stock."""
    invoice = store.invoice(invoice_id)
    if invoice.paid_cents:
        raise ValueError(f"{invoice_id} has payments; refund them first")
    if invoice.status != "void":
        for line in invoice.lines:
            inventory.release(store, line.sku, line.quantity)
        invoice.status = "void"
        store.save()
    return invoice
