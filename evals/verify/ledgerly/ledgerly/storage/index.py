"""Lookups over a loaded store, rebuilt after each change."""


class Index:
    def __init__(self, store):
        self.rebuild(store)

    def rebuild(self, store):
        self.customers = {customer.id: customer for customer in store.customers}
        self.products = {product.sku: product for product in store.products}
        self.invoices = {invoice.id: invoice for invoice in store.invoices}
        self.by_customer = {}
        for invoice in store.invoices:
            self.by_customer.setdefault(invoice.customer_id, []).append(invoice)
