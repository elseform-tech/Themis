"""`import FILE`: invoices exported by the old till (JSON list; see docs/CLI.md, "Import")."""

import json
import logging

from ledgerly.core import dates, ids, money
from ledgerly.models.invoice import Invoice, Line

log = logging.getLogger("ledgerly.import")


def register(groups):
    parser = groups.add_parser("import")
    parser.add_argument("file")
    parser.set_defaults(run=import_file)


def record_to_invoice(record, existing):
    """One till record -> an Invoice; a malformed record raises ValueError or KeyError."""
    issued = dates.parse_date(record["date"])
    lines = [Line(item["sku"], int(item["qty"]), money.to_cents(item["price"])) for item in record["lines"]]
    invoice_id = record.get("id") or ids.next_invoice_id(existing, issued.year)
    return Invoice(invoice_id, record["customer"], issued, lines, created_at=dates.utc_now())


def import_file(store, args, out):
    with open(args.file, encoding="utf-8") as handle:
        records = json.load(handle)
    imported = skipped = 0
    for record in records:
        try:
            invoice = record_to_invoice(record, store.index.invoices)
            store.customer(invoice.customer_id)
        except (KeyError, ValueError) as error:
            log.warning("skipped record %s: %s", record.get("id", "?"), error)
            skipped += 1
            continue
        if invoice.id in store.index.invoices:
            skipped += 1
            continue
        store.invoices.append(invoice)
        store.index.invoices[invoice.id] = invoice
        imported += 1
    store.save()
    print(f"imported {imported}, skipped {skipped}", file=out)
