"""Reports: the monthly summary and its CSV export."""

from ledgerly.core import dates, money
from ledgerly.services import pricing
# >>> plant:refactor-split-billing
from ledgerly.services import billing as payments
# === plant:refactor-split-billing
from ledgerly.services import payments
# <<< plant:refactor-split-billing


def invoices_in(store, month):
    return [invoice for invoice in store.invoices if dates.month_key(invoice.issued) == month]


def monthly_summary(store, month, rates=None):
    """Totals for invoices issued in `month` ("YYYY-MM"); void invoices count as zero."""
    summary = {"month": month, "invoices": 0, "total_cents": 0, "paid_cents": 0,
               "outstanding_cents": 0, "by_customer": {}}
    for invoice in invoices_in(store, month):
        customer = store.customer(invoice.customer_id)
        total = pricing.total(invoice, customer, store.index.products, rates)
        summary["invoices"] += 1
        summary["total_cents"] += total
        summary["paid_cents"] += invoice.paid_cents
        summary["outstanding_cents"] += payments.balance(store, invoice, rates) if invoice.status == "open" else 0
        summary["by_customer"][customer.id] = summary["by_customer"].get(customer.id, 0) + total
    return summary


def to_csv(store, month, rates=None):
    """The month's invoices as CSV text, one row per invoice."""
# >>> plant:feature-discount-codes
    rows = ["id,customer,issued,subtotal,tax,total,status"]
# === plant:feature-discount-codes
    rows = ["id,customer,issued,subtotal,discount,code,tax,total,status"]
# <<< plant:feature-discount-codes
    for invoice in invoices_in(store, month):
        customer = store.customer(invoice.customer_id)
        products = store.index.products
        cells = [invoice.id, customer.id, invoice.issued.isoformat(), str(pricing.subtotal(invoice))]
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
        cells += [str(pricing.discount(invoice)), invoice.discount_code or ""]
# <<< plant:feature-discount-codes
        cells += [str(pricing.tax(invoice, customer, products, rates)),
                  str(pricing.total(invoice, customer, products, rates)), invoice.status]
        rows.append(",".join(cells))
    return "\n".join(rows) + "\n"


# >>> plant:feat-top
# === plant:feat-top
def top_customers(store, month, limit=5, rates=None):
    """[(customer, total_cents)] for the customers with the highest invoiced total in `month`
    (void invoices count as zero), highest first; equal totals by customer id."""
    totals = monthly_summary(store, month, rates)["by_customer"]
    ranked = sorted(((store.customer(cid), cents) for cid, cents in totals.items() if cents > 0),
                    key=lambda pair: (-pair[1], pair[0].id))
    return ranked[:limit]


# <<< plant:feat-top
def describe(summary):
    """A short text version of a monthly summary for the CLI."""
    lines = [f"{summary['month']}: {summary['invoices']} invoices, total {money.format_cents(summary['total_cents'])}",
             f"  paid {money.format_cents(summary['paid_cents'])}, outstanding {money.format_cents(summary['outstanding_cents'])}"]
    for customer, cents in sorted(summary["by_customer"].items()):
        lines.append(f"  {customer}: {money.format_cents(cents)}")
    return "\n".join(lines)
