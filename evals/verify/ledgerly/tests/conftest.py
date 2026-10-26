import pytest

from ledgerly.models.customer import Customer
from ledgerly.models.product import Product
from ledgerly.models.stock import StockLevel
from ledgerly.storage.json_store import Store


@pytest.fixture
def store(tmp_path):
    store = Store(str(tmp_path / "ledgerly.json"))
    store.customers = [Customer("c1", "Hartmann GmbH", "DE", 14), Customer("c2", "Atelier Roux", "FR", 30)]
    store.products = [Product("PEN", "Fountain pen", 250), Product("BOOK", "Notebook A5", 1200),
                      Product("GIFT", "Gift card", 5000, taxable=False)]
    store.stock = [StockLevel("PEN", 100), StockLevel("BOOK", 40), StockLevel("GIFT", 1000)]
    store.save()
    return store
