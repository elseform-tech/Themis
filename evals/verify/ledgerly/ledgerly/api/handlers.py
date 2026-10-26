"""JSON handlers. Field names are the API contract in docs/ARCHITECTURE.md ("API")."""

from ledgerly.core import money
from ledgerly.services import pricing
# >>> plant:refactor-split-billing
from ledgerly.services import billing as invoicing
# === plant:refactor-split-billing
from ledgerly.services import invoicing
# <<< plant:refactor-split-billing
# >>> plant:refactor-split-billing
from ledgerly.services import billing as payments
# === plant:refactor-split-billing
from ledgerly.services import payments
# <<< plant:refactor-split-billing


def invoice_view(store, invoice, rates=None):
    customer = store.customer(invoice.customer_id)
    total = pricing.total(invoice, customer, store.index.products, rates)
    return {"id": invoice.id, "customer": customer.name, "issued": invoice.issued.isoformat(),
            "status": invoice.status,
# >>> plant:web-mismatch
            # 0.9: shorter names for the mobile client
            "total": total, "paid": invoice.paid_cents,
            "balance": payments.balance(store, invoice, rates)}
# === plant:web-mismatch
            "total_cents": total, "paid_cents": invoice.paid_cents,
            "balance_cents": payments.balance(store, invoice, rates)}
# <<< plant:web-mismatch


def list_invoices(store, query=None):
    """Every invoice, newest first; `status` filters."""
    status = (query or {}).get("status")
    chosen = [invoice for invoice in store.invoices if not status or invoice.status == status]
    chosen.sort(key=lambda invoice: (invoice.issued, invoice.id), reverse=True)
    return {"invoices": [invoice_view(store, invoice) for invoice in chosen]}


def create_invoice(store, payload):
    """{"customer": id, "lines": [{"sku", "qty", "price"?}]}; prices are euro amounts as text."""
    items = []
    for line in payload.get("lines", []):
        item = (line["sku"], int(line["qty"]))
        if line.get("price") is not None:
            item += (money.to_cents(line["price"]),)
        items.append(item)
    invoice = invoicing.issue_invoice(store, payload["customer"], items)
    return invoice_view(store, invoice)


def pay(store, invoice_id, payload):
    invoice = payments.record_payment(store, invoice_id, money.to_cents(payload["amount"]))
    return invoice_view(store, invoice)
