"""`invoices create|pay|void|show`."""

from ledgerly.core import money
from ledgerly.services import pricing, tax
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


def parse_item(text):
    """"SKU:QTY" or "SKU:QTY:PRICE" (price in euros, e.g. 4.35)."""
    parts = text.split(":")
    if len(parts) not in (2, 3):
        raise ValueError(f"expected SKU:QTY[:PRICE], got {text!r}")
    item = (parts[0], int(parts[1]))
    return item + (money.to_cents(parts[2]),) if len(parts) == 3 else item


def register(groups):
    parser = groups.add_parser("invoices")
    actions = parser.add_subparsers(dest="action", required=True)
    create = actions.add_parser("create")
    create.add_argument("--customer", required=True)
    create.add_argument("--item", action="append", required=True, type=parse_item)
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
    create.add_argument("--code", help="a discount code (see docs/CLI.md)")
# <<< plant:feature-discount-codes
    create.set_defaults(run=create_invoice)
    pay = actions.add_parser("pay")
    pay.add_argument("invoice")
    pay.add_argument("amount", type=money.to_cents)
    pay.set_defaults(run=pay_invoice)
    void = actions.add_parser("void")
    void.add_argument("invoice")
    void.set_defaults(run=void_invoice)
    show = actions.add_parser("show")
    show.add_argument("invoice")
    show.set_defaults(run=show_invoice)


def _rates(args):
    return tax.load_rates(args.rates) if args.rates else None


def create_invoice(store, args, out):
# >>> plant:feature-discount-codes
    invoice = invoicing.issue_invoice(store, args.customer, args.item)
# === plant:feature-discount-codes
    invoice = invoicing.issue_invoice(store, args.customer, args.item, code=args.code)
# <<< plant:feature-discount-codes
    customer = store.customer(invoice.customer_id)
    total = pricing.total(invoice, customer, store.index.products, _rates(args))
    print(f"{invoice.id}\t{money.format_cents(total)}", file=out)


def pay_invoice(store, args, out):
    invoice = payments.record_payment(store, args.invoice, args.amount, _rates(args))
    print(f"{invoice.id}\t{invoice.status}\towes {money.format_cents(payments.balance(store, invoice, _rates(args)))}", file=out)


def void_invoice(store, args, out):
    invoice = invoicing.void_invoice(store, args.invoice)
    print(f"{invoice.id}\t{invoice.status}", file=out)


def show_invoice(store, args, out):
    invoice = store.invoice(args.invoice)
    customer = store.customer(invoice.customer_id)
    products = store.index.products
    rates = _rates(args)
    print(f"{invoice.id} for {customer.name} ({customer.id}), issued {invoice.issued}, {invoice.status}", file=out)
    for line in invoice.lines:
        print(f"  {line.sku} x{line.quantity} @ {money.format_cents(line.unit_cents)}", file=out)
    print(f"  subtotal {money.format_cents(pricing.subtotal(invoice))}", file=out)
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
    if invoice.discount_code:
        print(f"  discount {invoice.discount_code} -{money.format_cents(pricing.discount(invoice))}", file=out)
# <<< plant:feature-discount-codes
    print(f"  tax {money.format_cents(pricing.tax(invoice, customer, products, rates))}", file=out)
    print(f"  total {money.format_cents(pricing.total(invoice, customer, products, rates))}", file=out)
