# Summary packing and long-chat history transport

This is local implementation and regression evidence. The sequential real-model
200k/256k/24k acceptance matrix remains incomplete. Neither cached document
preparation nor mocked checkpoints substitute for the required live checkpoints.

## Observed live failure

The single durable test chat accumulated 843 history rows containing 8,530,400
serialized UTF-8 bytes. Its largest row was 176,924 bytes. The legacy full-history
response exceeded the shared server's 8,388,608-byte frame limit; it failed before
writing a frame header, producing CLI `unexpected end of file`. The observer
requested Stop, and the chat was subsequently confirmed idle. This was not a
successful narrative trial or evidence of a provider context overflow.

The stopped run's final durable accounting retains all 47 request attempts
(38 ordinary, nine summary), 37 tool calls and zero new checkpoints. Known
provider usage is 4,016,673 input and 31,437 output tokens, with three attempts
missing usage. All ordinary estimates fit 200k including output reserve; all 37
ordinary attempts with reported input usage also fit. Missing usage is unknown.

## Changes and regression evidence

- Source packets now fit the complete constructed summary request, reserving
  retry output and prompt overhead. Ordered UTF-8 ranges retain every eligible
  serialized source byte. A prose regression that previously required four
  source packets now permits at most two at 200k. Live efficiency gains remain
  unmeasured. Stage cache keys bind resolved model and output policy; ambiguous
  older partial keys are retained without speculative reuse.
- `get_thread_history_page` reads exact JSONL through read-only incremental
  SQLite BLOB I/O over the existing TEXT column. A fixed sequence watermark and
  byte cursor support appends and restart continuation. Shared native/CLI clients
  preserve the existing full-history result by reassembling pages.
- Oversized ordinary RPC responses return a bounded actionable error. Oversized
  subscription events request history reconciliation through the existing
  `lagged` notification. Neither path silently disconnects at the size limit.
- A real shared-server/CLI regression reproduced the oversized-entry EOF before
  the fix and passed after paging. Unicode boundaries, row limits, escaped wire
  size, snapshot appends, restart continuation and invalid cursors are covered.
  The stricter SQLite regression stores an 8 MiB entry, limits materialized SQL
  values to 4 KiB, then reads a 128-byte page. SQL slicing failed this test;
  incremental I/O passes. Full-history client/UI memory still grows with history.

Independent reviewer `review_adaptive_compaction` cleared the final incremental
read implementation and summary-packing/cache changes. Fresh local checks passed:
workspace Clippy with warnings denied, formatting/diff checks, 14 dashboard tests,
Ruff, 36 offline acceptance-harness tests, 200 core unit tests, and the three
focused incremental history-page tests. The final command
`cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`
exited zero: 462 passed, three ignored and one credential-mutating test deliberately
excluded. The ignored cases are the public-repository download acceptance, the
standalone live-provider smoke test and the tools documentation example. The
sequential custom live acceptance matrix is separate and remains incomplete.
The rerun includes the advanced-endpoint inventory correction discovered by the
earlier desktop suite.

## Health and remaining proof

The refreshed dashboard analyzes 116 product files: mean cyclomatic complexity
2.56, cognitive complexity 2.20 and maintainability 73.1. File means for summary
jobs are 2.93/2.00/62.1, history pages 2.24/1.11/78.1, the server 3.27/0.55/78.3,
and thread handlers 2.15/0.80/79.1. Inline Rust tests remain included. Existing
Rust line coverage is stale at 89.1%; changed-line coverage and frontend coverage
are unavailable. These averages do not establish correctness or impose a gate.

The history patch still needs its focused commit, packaged rebuild/install and
the same-chat live rerun. Native visible verification
is unavailable while the Mac is locked. Narrative semantic/source acceptance,
autonomous large multi-file coding, continuity/topic recall and genuine forced
checkpoints at all three budgets remain required. All prior failed-attempt costs
and immutable sources are retained.
