"""VAT by customer country, from data/rates.csv ("country,percent")."""

import csv
from decimal import Decimal

from ledgerly.core import money

DEFAULT_RATES = {"DE": Decimal("19"), "FR": Decimal("20"), "NL": Decimal("21"), "AT": Decimal("20")}


def load_rates(path):
    """{country: percent} from a CSV file with a header row."""
    rates = {}
    with open(path, encoding="utf-8", newline="") as handle:
        for row in csv.DictReader(handle):
            rates[row["country"].strip().upper()] = Decimal(row["percent"].strip())
    return rates


def rate_for(country, rates=None):
    rates = DEFAULT_RATES if rates is None else rates
    try:
        return rates[country.upper()]
    except KeyError:
        raise KeyError(f"no VAT rate for {country!r}") from None


def tax_cents(taxable_cents, country, rates=None):
    return money.percent_of(taxable_cents, rate_for(country, rates))
