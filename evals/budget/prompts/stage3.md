Finally, add a command-line report: `python3 -m ledger report DATA_DIR --today YYYY-MM-DD`.

- For every desk in `ledger.desks.DESKS`, read `DATA_DIR/<desk module name>.csv` (header `id,account,amount,currency,date`; amounts are integer cents). A desk without a file has no entries.
- Settle the entries with the desk's `settle` and work out holds with its `holds` at `--today`.
- Print one line per desk, sorted by desk code (the code on each desk's page):
  `<CODE> lines=<accepted> rejected=<rejected> fees=<total fees> released=<released> held=<held>`
  where fees are written in currency units with two decimals (12345 cents is `123.45`).
- Then one last line with the sums over all desks:
  `TOTAL desks=<number of desks> lines=<n> rejected=<n> fees=<total> released=<n> held=<n>`
- Print nothing else to stdout and exit with status 0. A `--today` that is not a valid date prints an error to stderr, nothing to stdout, and exits with status 2.

Add tests, make sure every test passes, including the earlier ones, and tell me briefly what the service now does.