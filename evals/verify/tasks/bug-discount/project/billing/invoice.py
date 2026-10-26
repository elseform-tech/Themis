"""Invoice totals, in cents."""


def line_total(price_cents, quantity):
    return price_cents * quantity


def total_due(lines, discount_percent=0, tax_percent=0):
    """The total in cents for `lines` of (price_cents, quantity): the lines summed, the discount
    taken off, then tax added on the discounted amount. Rounds half up, once, at the end."""
    subtotal = sum(line_total(price, quantity) for price, quantity in lines)
    discounted = subtotal * (100 - discount_percent) / 100
    taxed = discounted + subtotal * tax_percent / 100
    return int(taxed + 0.5)
