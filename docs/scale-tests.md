# Scale tests

Scale tests catch work that grows faster than a chat does. They count work, such as list items read or markdown parses, instead of timing it, so they give the same answer on every machine and can fail a pull request. Web tests use `web/src/test/scale.ts`, and they sit beside the code they cover as `*.scale.test.ts(x)`.

`growthRate` compares two sizes: 1 means the work grows with the chat, 2 means it grows with the square of the chat. `scale.test.ts` checks that it can tell those apart.

Write a scale test before fixing a slow path, and see it fail. Use the worst realistic input. Keep sizes at or below 8,000 messages, so a slow path fails in seconds and doesn't tie up the machine. If a test passes the first time, the path was fine: keep the test if it's cheap, and don't change the code.

## Rust

Rust scale tests use `crates/desktop/src/scale.rs`. `growth_rate` works as it does on the web, with 0 meaning the work stays flat. `measure` runs the sizes smallest first and gives each one 30 seconds. That limit protects the machine and is the only clock involved: past it, the measurement is abandoned, larger sizes are skipped and the test judges what finished.

Two counters show the work without changing the code they measure:

- `sqlite_steps` in `crates/desktop/src/transcript/scale_tests.rs` uses SQLite's progress hook, which sees every statement on the connection. The hook is a `rusqlite` feature turned on for tests only.
- `take_work` in `crates/desktop/src/diff.rs` returns the files opened, bytes read, diffs built and items looked at on the current thread. The `count_work!` lines that feed it compile away outside tests. Read it inside the closure you give `measure`, because each measurement runs on its own thread.

To fill a chat quickly, turn off `synchronous` on the test's own store, as `chat` does. Run these tests with `-j 4` and a name filter while working on them.

## What the tests cover

| Path | Test | Bound |
| --- | --- | --- |
| Attachment list at the start of a run | `attachment_lookup_stays_flat…` | Flat: the list is stored and kept up to date on append |
| Recall search on each send | `recall_reads_a_chat_once…`, `recall_stays_linear…` | One read of the chat however many terms the request has, off the async task. It stays linear, because finding a match in old turns means reading them |
| One page of history | `one_history_page_costs_the_same…` | Flat |
| Change fingerprints at each end of a run | `fingerprint_reads_do_not_grow…` | At most 1 MiB per file. Above that a file is judged by its size, modified time and first and last 64 KiB, so saving it unchanged counts as an edit, and an edit to its middle that keeps its size and modified time goes unseen |
| Diff listing, every 1.5 seconds | `listing_many_hidden_files_stays_linear`, `listing_builds_no_more_diffs_than_it_shows` | One short look at each new file for the counts, and full reads only for the 200 shown |
| Replaying a chat's history in the web app | `replaying a chat's history stays linear…` | Linear: the list being built grows in place |

The diff listing still runs `git diff` over every tracked change and still counts the lines of each shown new file in full, so a very large new file costs a full read on each listing.

Timing measurements of growth are benchmark audits: they report a trend and don't fail a pull request.
