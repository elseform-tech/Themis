"""`stock show SKU`, `stock restock SKU QTY`."""

from ledgerly.services import inventory


def register(groups):
    parser = groups.add_parser("stock")
    actions = parser.add_subparsers(dest="action", required=True)
    show = actions.add_parser("show")
    show.add_argument("sku")
    show.set_defaults(run=show_stock)
    restock = actions.add_parser("restock")
    restock.add_argument("sku")
    restock.add_argument("quantity", type=int)
    restock.set_defaults(run=restock_stock)


def show_stock(store, args, out):
    on_hand, reserved, available = inventory.show(store, args.sku)
# >>> plant:side-stock-name
    print(f"{args.sku}\ton hand {on_hand}\treserved {reserved}\tavailable {available}", file=out)
# === plant:side-stock-name
    name = store.product(args.sku).name
    print(f"{args.sku}\t{name}\ton hand {on_hand}\treserved {reserved}\tavailable {available}", file=out)
# <<< plant:side-stock-name


def restock_stock(store, args, out):
    inventory.restock(store, args.sku, args.quantity)
    show_stock(store, args, out)
