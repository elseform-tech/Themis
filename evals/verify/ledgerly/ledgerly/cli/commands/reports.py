"""`reports monthly MONTH [--csv]`, `reports overdue`, `reports aging`."""

from ledgerly.core import money
from ledgerly.services import reports, tax
# >>> plant:refactor-split-billing
from ledgerly.services import billing as payments
# === plant:refactor-split-billing
from ledgerly.services import payments
# <<< plant:refactor-split-billing


def register(groups):
    parser = groups.add_parser("reports")
    actions = parser.add_subparsers(dest="action", required=True)
    monthly = actions.add_parser("monthly")
    monthly.add_argument("month", help="YYYY-MM")
    monthly.add_argument("--csv", action="store_true")
    monthly.set_defaults(run=monthly_report)
    actions.add_parser("overdue").set_defaults(run=overdue_report)
    actions.add_parser("aging").set_defaults(run=aging_report)


def monthly_report(store, args, out):
    rates = tax.load_rates(args.rates) if args.rates else None
    if args.csv:
        out.write(reports.to_csv(store, args.month, rates))
    else:
        print(reports.describe(reports.monthly_summary(store, args.month, rates)), file=out)


def overdue_report(store, args, out):
    for invoice in payments.overdue(store):
        owed = payments.balance(store, invoice)
        print(f"{invoice.id}\t{invoice.customer_id}\tdue {payments.due_date(store, invoice)}\t{money.format_cents(owed)}", file=out)


def aging_report(store, args, out):
    for bucket, ids in payments.aging(store).items():
        print(f"{bucket}\t{len(ids)}\t{' '.join(ids)}", file=out)
