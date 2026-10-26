"""Invoice amounts: subtotal, discount, tax and total, all in cents."""

# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
from ledgerly.services import discounts
# <<< plant:feature-discount-codes
from ledgerly.services import tax as vat


def line_total(line):
    return line.quantity * line.unit_cents


def subtotal(invoice):
    return sum(line_total(line) for line in invoice.lines)


def discount(invoice):
# >>> plant:feature-discount-codes
    return 0
# === plant:feature-discount-codes
    return discounts.discount_cents(subtotal(invoice), invoice.discount_code)
# <<< plant:feature-discount-codes


def taxable(invoice, products):
    """The taxable part after the discount, which applies across all lines in proportion."""
    base = subtotal(invoice)
    if base == 0:
        return 0
    taxed = sum(line_total(line) for line in invoice.lines if products[line.sku].taxable)
    return taxed - (discount(invoice) * taxed) // base


def tax(invoice, customer, products, rates=None):
    return vat.tax_cents(taxable(invoice, products), customer.country, rates)


def total(invoice, customer, products, rates=None):
    """What the customer owes for the invoice in all."""
    if invoice.status == "void":
        return 0
    return subtotal(invoice) - discount(invoice) + tax(invoice, customer, products, rates)
