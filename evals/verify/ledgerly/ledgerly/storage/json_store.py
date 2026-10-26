"""The JSON store: customers, products, invoices and stock in one file, written atomically."""

import json
import os
import tempfile

from ledgerly.models.customer import Customer
from ledgerly.models.invoice import Invoice
from ledgerly.models.product import Product
from ledgerly.models.stock import StockLevel
from ledgerly.storage import migrations
from ledgerly.storage.index import Index


class Store:
    def __init__(self, path):
        self.path = path
        self.customers, self.products, self.invoices, self.stock = [], [], [], []
        self.index = Index(self)

    @classmethod
    def load(cls, path):
        store = cls(path)
        if os.path.exists(path):
            with open(path, encoding="utf-8") as handle:
                data = migrations.migrate(json.load(handle))
            store.customers = [Customer.from_dict(item) for item in data.get("customers", [])]
            store.products = [Product.from_dict(item) for item in data.get("products", [])]
            store.invoices = [Invoice.from_dict(item) for item in data.get("invoices", [])]
            store.stock = [StockLevel.from_dict(item) for item in data.get("stock", [])]
        store.index.rebuild(store)
        return store

    def save(self):
        data = {"schema": migrations.CURRENT_SCHEMA,
                "customers": [item.to_dict() for item in self.customers],
                "products": [item.to_dict() for item in self.products],
                "invoices": [item.to_dict() for item in self.invoices],
                "stock": [item.to_dict() for item in self.stock]}
        folder = os.path.dirname(os.path.abspath(self.path))
        handle, temporary = tempfile.mkstemp(dir=folder, suffix=".tmp")
        with os.fdopen(handle, "w", encoding="utf-8") as out:
            json.dump(data, out, indent=1, sort_keys=True)
        os.replace(temporary, self.path)
        self.index.rebuild(self)

    def customer(self, customer_id):
        try:
            return self.index.customers[customer_id]
        except KeyError:
            raise KeyError(f"unknown customer {customer_id!r}") from None

    def product(self, sku):
        try:
            return self.index.products[sku]
        except KeyError:
            raise KeyError(f"unknown product {sku!r}") from None

    def invoice(self, invoice_id):
        try:
            return self.index.invoices[invoice_id]
        except KeyError:
            raise KeyError(f"unknown invoice {invoice_id!r}") from None

    def stock_of(self, sku):
        for level in self.stock:
            if level.sku == sku:
                return level
        level = StockLevel(sku)
        self.stock.append(level)
        return level
