Two changes, for every desk.

1. The minimum fee of every desk in settlement tier {{tier}} is now {{new_min}} cents. The desk pages still show the old minimum; do not edit the docs. Desks in other tiers keep their minimum fee.
2. Add `holds(entries, today)` to every desk module. `today` is a `YYYY-MM-DD` string. It returns `{"released": [...], "held": [...]}` for the entries the desk would accept (rejected entries appear in neither list), following the Holds section of docs/overview.md and the hold period and calendar on each desk's page. An entry is released when its release date is on or before `today`. `released` lists entry ids in the desk's line order. `held` lists `{"id": ..., "until": "YYYY-MM-DD"}` (the release date), sorted by `until`, then by id.

Keep everything that already works, add tests, and run the whole test suite.