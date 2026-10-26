# plant-file:refactor-split-billing only
"""Billing: issuing and voiding invoices, payments, balances and what is overdue."""

from ledgerly.core import dates, ids
from ledgerly.models.invoice import Invoice, Line
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
from ledgerly.services import discounts
# <<< plant:feature-discount-codes
from ledgerly.services import inventory, pricing


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


def balance(store, invoice, rates=None):
    """Cents still owed on `invoice`."""
    customer = store.customer(invoice.customer_id)
    return pricing.total(invoice, customer, store.index.products, rates) - invoice.paid_cents


def record_payment(store, invoice_id, cents, rates=None):
    """Records a payment; an invoice paid in full is closed and its stock shipped."""
    if cents <= 0:
        raise ValueError("a payment must be positive")
    invoice = store.invoice(invoice_id)
    if invoice.status != "open":
        raise ValueError(f"{invoice_id} is {invoice.status}")
    owed = balance(store, invoice, rates)
    if cents > owed:
        raise ValueError(f"{invoice_id} owes {owed} cents; {cents} is too much")
    invoice.paid_cents += cents
    if cents == owed:
        invoice.status = "paid"
        for line in invoice.lines:
            inventory.ship(store, line.sku, line.quantity)
    store.save()
    return invoice


def due_date(store, invoice):
    return dates.add_days(invoice.issued, store.customer(invoice.customer_id).terms_days)


def overdue(store, now=None):
    """Open invoices past their due date, oldest first."""
    late = [invoice for invoice in store.invoices
            if invoice.status == "open" and dates.is_overdue(due_date(store, invoice), now)]
    return sorted(late, key=lambda invoice: invoice.issued)


def aging(store, now=None):
    """Open invoices by days since they were created: {"0-30": [...], "31-60": [...], "61+": [...]}."""
    now = now or dates.utc_now()
    buckets = {"0-30": [], "31-60": [], "61+": []}
    for invoice in store.invoices:
        if invoice.status != "open" or invoice.created_at is None:
            continue
        days = (now - invoice.created_at).days
        key = "0-30" if days <= 30 else "31-60" if days <= 60 else "61+"
        buckets[key].append(invoice.id)
    return buckets
