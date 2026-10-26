"""Stock: reserved when invoiced, shipped when paid, released when voided."""


def reserve(store, sku, quantity):
    store.product(sku)
    level = store.stock_of(sku)
    if level.available < quantity:
        raise ValueError(f"only {level.available} of {sku} available")
    level.reserved += quantity


def release(store, sku, quantity):
    level = store.stock_of(sku)
    level.reserved = max(0, level.reserved - quantity)


def ship(store, sku, quantity):
    level = store.stock_of(sku)
    level.reserved = max(0, level.reserved - quantity)
    level.on_hand -= quantity


def restock(store, sku, quantity):
    if quantity <= 0:
        raise ValueError("restock a positive quantity")
    store.product(sku)
    store.stock_of(sku).on_hand += quantity
    store.save()


def show(store, sku):
    """(on hand, reserved, available) for `sku`."""
    level = store.stock_of(store.product(sku).sku)
    return level.on_hand, level.reserved, level.available
