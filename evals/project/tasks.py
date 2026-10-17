"""The five messages of the project job. The rules for the whole job appear only in the first."""

from .grade import bug_report, cents

FEATURE = """In this project ({pkg}), add discount rules.

Rules file format, one rule per line (blank lines and lines starting with '#' are ignored):
- `percent <category> <whole percent from 1 to 100>`: that percent off every line of that category,
  rounded down to whole cents.
- `fixed <sku> <amount, like 1.50>`: that amount off each unit of that product; a line never goes below 0.
- `threshold <minimum amount> <amount off>`: that amount off the whole order when the order's discounted
  subtotal is at least the minimum. Only the largest qualifying threshold counts, and never more than
  that subtotal.
If several percent rules (or several fixed rules) apply to the same line, only the largest of that kind
counts.

How discounts apply: each product line (its total after bundles) gets the larger of its percent discount
and its fixed discount, not both. Tax is charged on each line's total after its discount. Then the
threshold discount applies to the order and does not change the tax.

API:
- New `{pkg}/discounts.py` with `parse_discounts(text)` returning a list of rules. Any malformed line
  (unknown kind, wrong number of fields, bad number, percent outside 1-100, an amount that is zero or
  negative) raises a new `DiscountError` in `{pkg}/errors.py`: a `PricingError` with a `line` attribute
  holding the one-based line number.
- `price_order(order, catalog, rules=())`; existing calls must keep working.
- `Quote` gets `discount_cents` (default 0). `subtotal_cents` stays the sum of line totals before
  discounts, `discount_cents` is all discounts together, and `total_cents` = subtotal - discount + tax.

Rules for this whole job, in every message: money is integer cents everywhere (no floats, no Decimal,
no `/` division in package code); never change anything under tests/legacy/; every new public function
or class gets a docstring.

For now, do the rules parser and the pricing, with tests in tests/test_discounts.py, and run the test
suite. Do not change the CLI, the report or the docs yet: stop when the parser and pricing are done and
wait for me."""

PROGRESS = ("Progress check, do not change anything: which parts of the discount work are done, which are "
            "left, and how many test methods have you added so far?")

BUG = """A customer was overcharged. With data/catalog.csv, this order was quoted a total of {charged}, but it
should have been {expected}:

```
{order}```

Find the cause, fix it, add a regression test, and keep everything else working. Do not start on the CLI
yet."""

FINISH = ("Now finish the discount feature: the CLI `quote` command takes an optional `--discounts FILE`; "
          "the report prints a line `Discounts: -<amount>` between Subtotal and Tax when the discount is above "
          "zero (no line otherwise); saved JSON quotes keep `discount_cents`, and older files without it still "
          "load; and docs/pricing.md gets a Discounts section explaining the three rule kinds. Run the whole "
          "test suite before you finish.")

RECALL = ("Remind me: what rules did I give you for this whole job at the start, and what changed in this "
          "chat overall?")


def prompts(facts):
    report = bug_report(facts)
    return [FEATURE.format(pkg=facts["package"]), PROGRESS,
            BUG.format(charged=cents(report["charged"]), expected=cents(report["expected"]), order=report["order"]),
            FINISH, RECALL]
