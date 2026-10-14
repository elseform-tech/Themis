# Recall tool verification (2026-10-14)

Branch `codex/recall-tool` vs `origin/main` (5e2a489). Automatic checks and live
measurements are labelled separately. Live numbers come from a free, nondeterministic
model with very small samples; treat them as evidence about behaviour, not a benchmark.

## 1. Prompt overhead (deterministic, measured from source)

Typical directory `/Users/me/proj/.themis/context/<36-char uuid>`, byte counts of the
literal text the runtime appends (tokens are bytes/4, the runtime's own estimate):

| Text | main | branch | change |
| --- | --- | --- | --- |
| Per-request system evidence text (evidence-directory manual + shell/Python paragraph) | 3,619 B (~904 tok) | 692 B (~173 tok) with `recall`; 766 B in the read_file fallback | -2,927 B (-2,853 B fallback) |
| Tool definition for `recall` (name, description, schema) | 0 | 878 B (~220 tok) | +878 B |
| Net per request | | | -2,049 B (~-512 tok) |
| Checkpoint "Recoverable evidence" block | 1,554 B (~388 tok) | 728 B (~182 tok) | -826 B per checkpoint |

The system text grew from the first cut (374 B) after review: it now also carries the
staleness and tool-restriction guidance and a receipt-reopen line from the removed manual,
and a "call recall first, say so if nothing is found" rule. The tool definition was
trimmed (1,103 B to 878 B) to keep the net saving above 2 KB.

Live confirmation from the runtime's own `context_budget` event (system section,
estimated tokens, first request of a run, measured before the guidance was restored): main 2,510-2,511,
branch 1,698-1,701 (the tool schema is counted separately).
Target (>= 2 KB less per request, net of the new tool definition) is met.

## 2. Automatic checks

Run from the repository root (results are from this branch at the final commit):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten
(cd fuzz && cargo +nightly fuzz build --sanitizer none)
(cd fuzz && cargo +nightly fuzz run --sanitizer none recall_search -- -max_total_time=45)
```

New tests: `crates/core/src/tools/recall/tests.rs` (unit: ranking, dedupe, UTF-8,
decoding, kinds, confinement incl. symlinks, around, budgets, chunk boundary; proptest:
record parser, bounded line reader, scan ranges, decoding, folding),
`crates/core/tests/recall_e2e.rs` (scripted provider: forced compaction, then `recall`
finds a planted original, coordinates and SHA-256 validated against the saved snapshot),
`crates/desktop/tests/recall_e2e.rs` (shared server + CLI in an isolated data dir: attachment
original found with valid coordinates, zero approval requests, no leftover server process),
fuzz target `fuzz/fuzz_targets/recall_search.rs` (131,779 runs in 46 s, no crash).

## 3. Live comparison (not CI)

Script: `docs/verification/recall-live.py`. Isolated `HOME` and `--data-dir` per run,
real model `longcat-2.5-preview-free`, Custom approval mode (default), user
`runtime.jsonc` = `{"context_token_budget":24000,"recent_messages":4,"total_turns":24}`.
Scenario: paste a 42,786-byte deterministic 12-chapter story with five planted facts,
paste ~17 KB of unrelated log (forces one checkpoint), then ask five detail questions in
separate sends. Prompts ask for the quote and the saved file name and record number.

```sh
cargo build -p themis-desktop --bin themis   # branch binary
# main binary: git worktree of origin/main in a scratch dir, same build command
python3 docs/verification/recall-live.py <binary> <label> <out.json> --scratch <dir>
```

"Cited coordinates valid" is the script's check: the answer names an existing saved
file and a record in it whose line contains the expected fact. In every final run the
checkpoint count after the filler step was 1.

### Final runs (42.8 KB story, two runs per side)

| Run | Q1 door number | Q2 ship | Q3 scarf colour | Q4 toll | Q5 key |
| --- | --- | --- | --- | --- | --- |
| main run 1 | wrong, 3 calls (no recall) | wrong, 0 | wrong, 0 | wrong, 0 | wrong, 0 |
| main run 2 | right, 25 | right, 9 | right, 4 | right, 2 | right, 2 |
| branch run 1 | right, 7 (recall used) | right, 1 (recall) | WRONG, 0 (invented quote) | WRONG, 3 (read_file/shell, no recall) | WRONG, 0 (invented) |
| branch run 2 | right, 4 (recall) | right, 5 (recall) | right, 2 (recall) | right, 2 (recall) | right, 2 (recall) |

Cells: answer correct?, tool calls. Citation validity (coordinates name an existing
saved file and a record containing the fact): valid in every correct branch answer; main
run 2 valid for 4 of 5 (Q1 had no parsable citation); main run 1 gave no citation
(it reported it could not find the story, which is an honest failure); branch run 1's
wrong answers cite a real snapshot/record but the quoted sentence is invented (the cited
record is the story, the quote does not appear in it, so correctness is scored wrong).

Totals: main 5/10 correct, median tool calls per question 2.5; branch 7/10 correct, `recall`
used on 7/10 questions (every question of run 2), median 2 calls per question.

Reading: when the model uses `recall`, a detail costs 1-5 calls and the answer is correct
with valid coordinates (7/7). The remaining branch failures are model compliance failures:
in run 1 the model answered Q3 and Q5 from memory with fabricated quotes and no tool call.
The shorter prompt does not force a search, and the old manual did not prevent the same
failure on main either (main run 1 gave up, run 2 flailed through 20+ read/list calls on Q1).
No claim of statistical significance is made; n is two runs per side.

### Exploratory runs (kept as raw JSON; earlier scenario versions)

Files `exploratory-*.json` use a 29 KB story. They are listed for transparency:

- `branch-run1`: before a fix. The model sent every optional field with a placeholder
  (`"around": "0"`), which the first implementation rejected as an invalid token; the model
  then flailed (45, 31, 3, 14 and 12 calls). Fixed by ignoring `around` values without a `|`
  token when a query is given (unit test `placeholder_around_values_do_not_block_a_search`).
  That run's filler step also exceeded the 24k budget (scenario error, filler later shrunk).
- `branch-run2-oldwording`: the first, non-imperative pointer. Q3 and Q4 answered from memory
  with invented quotes (3/5 correct). The pointer was made imperative afterwards.
- `branch-new1`, `branch-new2`: imperative wording: 5/5 each; run 1 used read_file/shell
  rather than recall (calls 13, 3, 1, 1, 1), run 2 used recall for every question (2, 1, 1, 1, 1).
- `main-run1`: 5/5 (calls 11, 1, 2, 3, 1); `main-run2`: 3/5 (calls 6, 3, 0, 0, 4).

### Follow-up runs after code review (this branch tip, 3 runs each)

Review asked for a stronger "recall first" rule. Wording now says: for any question about
the exact wording, numbers, names or quotes of earlier material, call `recall` first, answer
only from its result, and say so if nothing is found. Same 42.8 KB scenario, `review-*.json`:

| Batch | Correct | `recall` used | Answers with 0 tool calls | Valid citations |
| --- | --- | --- | --- | --- |
| r2 (rule + guidance, previous query description) | 10/15 | 9 | 1 | 14 |
| r3 (plus "2-5 distinctive keywords, not the whole question" in the tool schema) | 11/15 | 3 | 3 | 14 |
| earlier branch wording, 2 runs | 7/10 | 7 | 2 | 10 |
| main, 2 runs | 5/10 | 0 | 4 | 4 |

Honest reading: the stronger rule did not measurably reduce answers made from memory
(3 of 15 questions in r3 made no tool call and invented quotes; r2 had 1 of 15, earlier
branch wording 2 of 10). Correctness is about 70% on this branch in every batch versus 50% on
main (two runs), but samples are tiny and identical-prompt runs vary from 2/5 to 5/5, so the
difference is not statistically established. Inspecting the r2 runs showed two further effects: the
model often passes the whole question (plus the instruction suffix of the prompt) as the query, so
earlier copies of the question rank next to the real passage; and it can invent a name even after a
search (for example putting an invented answer in the `around` field). The keyword hint in the schema
did not visibly help either. No ranking change was made for this (inverse-frequency weighting
would be the next step), and nothing in the harness forces a search before an answer.

## 4. Limits and unverified

- The desktop UI was not exercised; the tool has no UI surface.
- Live evidence is two runs per side per scenario on one free model; variance between
  identical runs (0/5 vs 5/5 on main) is larger than the difference between sides.
- Models can still answer detail questions from memory without calling any tool; nothing in
  this change prevents that, and a fabricated answer can cite a real record.
- Attachment originals are searched only when the transcript records the attachment path for
  this chat and the file is UTF-8 up to 64 MiB; binary files, images and PDFs are not.
- Recalled text may include the active request itself when it shares words with the query.
