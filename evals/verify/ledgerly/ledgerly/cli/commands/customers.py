"""`customers list|add|show`."""

# >>> plant:feat-cshow
# === plant:feat-cshow
from ledgerly.core import money
# <<< plant:feat-cshow
from ledgerly.models.customer import Customer
# >>> plant:feat-cshow
# === plant:feat-cshow
# >>> plant:req-cshow-balance
# === plant:req-cshow-balance
# >>> plant:refactor-split-billing
from ledgerly.services import billing as payments
# === plant:refactor-split-billing
from ledgerly.services import payments
# <<< plant:refactor-split-billing
# <<< plant:req-cshow-balance
# <<< plant:feat-cshow


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
# >>> plant:feat-cshow
# === plant:feat-cshow
    show = actions.add_parser("show")
    show.add_argument("id")
    show.set_defaults(run=show_customer)
# <<< plant:feat-cshow


def list_customers(store, args, out):
    for customer in sorted(store.customers, key=lambda c: c.id):
        print(f"{customer.id}\t{customer.name}\t{customer.country}\t{customer.terms_days}d", file=out)


def add_customer(store, args, out):
    if args.id in store.index.customers:
        raise ValueError(f"customer {args.id} exists")
# >>> plant:side-country
    store.customers.append(Customer(args.id, args.name, args.country, args.terms))
# === plant:side-country
    store.customers.append(Customer(args.id, args.name, args.country.upper(), args.terms))
# <<< plant:side-country
    store.save()
    print(f"added {args.id}", file=out)
# >>> plant:feat-cshow
# === plant:feat-cshow


def show_customer(store, args, out):
    customer = store.customer(args.id)
    print(f"{customer.id}\t{customer.name}", file=out)
    print(f"  country {customer.country}", file=out)
# >>> plant:req-cshow-terms
    print(f"  terms {customer.terms_days}d", file=out)
# === plant:req-cshow-terms
    print(f"  terms {customer.terms_days} days", file=out)
# <<< plant:req-cshow-terms
# >>> plant:req-cshow-noemail
    print(f"  email {customer.email}", file=out)
# === plant:req-cshow-noemail
    print(f"  email {customer.email or 'no email'}", file=out)
# <<< plant:req-cshow-noemail
# >>> plant:req-cshow-balance
# === plant:req-cshow-balance
    open_invoices = [invoice for invoice in store.index.by_customer.get(customer.id, []) if invoice.status == "open"]
    owed = sum(payments.balance(store, invoice) for invoice in open_invoices)
    print(f"  open invoices {len(open_invoices)}, outstanding {money.format_cents(owed)}", file=out)
# <<< plant:req-cshow-balance
# <<< plant:feat-cshow
