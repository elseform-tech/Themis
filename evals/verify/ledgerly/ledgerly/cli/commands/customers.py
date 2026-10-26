"""`customers list|add`."""

from ledgerly.models.customer import Customer


def register(groups):
    parser = groups.add_parser("customers")
    actions = parser.add_subparsers(dest="action", required=True)
    actions.add_parser("list").set_defaults(run=list_customers)
    add = actions.add_parser("add")
    add.add_argument("id")
    add.add_argument("name")
    add.add_argument("--country", default="DE")
    add.add_argument("--terms", type=int, default=14)
    add.set_defaults(run=add_customer)


def list_customers(store, args, out):
    for customer in sorted(store.customers, key=lambda c: c.id):
        print(f"{customer.id}\t{customer.name}\t{customer.country}\t{customer.terms_days}d", file=out)


def add_customer(store, args, out):
    if args.id in store.index.customers:
        raise ValueError(f"customer {args.id} exists")
    store.customers.append(Customer(args.id, args.name, args.country.upper(), args.terms))
    store.save()
    print(f"added {args.id}", file=out)
