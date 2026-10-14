# Property tests and fuzzing

Themis parses a lot of input it does not control: model tool arguments and
provider responses, Git output, checkpoint history, attachment text and
paths from the UI. Two layers check that this code holds its invariants for
any input.

## Property tests (every CI run)

`proptest` (Rust) and `fast-check` (web) generate hundreds of inputs per test
inside the normal suites, on every platform, with reproducible shrinking.

| Area | Invariant | Location |
| --- | --- | --- |
| Provider wire | Any model tool arguments reach every protocol as a JSON object; valid objects unchanged, others wrapped with the exact original text | `crates/core/src/providers.rs` |
| Task ledger | User text never becomes ledger entries; rendered ledgers stay within budget, framed, one JSON object per line; plan arguments never panic | `crates/core/src/runtime/ledger.rs` |
| Truncation | Prefixes keep whole characters; summaries stay one bounded line | `crates/core/src/runtime.rs` |
| Project paths | No mix of names, `.`/`..`, separators or symlinks resolves outside the project | `crates/desktop/src/state/workspace.rs`, `state/changes.rs` |
| Diffs | Arbitrary text never panics the parser; well-formed diffs round-trip; change detection reports exactly the differing files; readable files are NUL-free UTF-8 | `crates/desktop/src/diff.rs` |
| Social messages | Any UI text never panics greeting detection or recall terms; terms bounded and sorted; social-only input is social (also repeated) with no terms; any non-social ASCII word makes it a task | `crates/core/src/social.rs` |
| Web state | Any event stream keeps running state consistent and attaches edited files to the right reply; prompt tokens rebuild exactly | `web/src/state/reducer.property.test.ts` |

## Fuzz targets (nightly CI)

`fuzz/` is a separate cargo-fuzz crate (outside the workspace) that reaches
internal parsers through the off-by-default `fuzzing` feature of
`themis-core` and `themis-desktop`.

| Target | Input |
| --- | --- |
| `diff_parse` | Raw `git diff` text |
| `project_path` | Paths against a project with an escaping symlink |
| `ledger_restore` | Checkpoint history, tool name, arguments and output, budget |
| `provider_wire` | Model tool arguments and provider JSON for all response/stream converters |
| `social_request` | UI message text for greeting/thanks detection and recall terms: bounded output, a non-social word always makes a task |

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked
cd fuzz
cargo +nightly fuzz build --sanitizer none
cargo +nightly fuzz run --sanitizer none diff_parse -- -max_total_time=60
```

Targets run without AddressSanitizer: they cover safe Rust for panics and
invariant violations, and the sanitizer fails to link Tauri dependencies on
macOS. The `fuzz` workflow builds every target on pull requests and runs each
for five minutes nightly (or on demand), uploading `fuzz/artifacts` on failure.

When a crash is found, minimise it (`cargo fuzz tmin`), fix the bug, and commit
the input as an ordinary regression test next to the code it exercises.

The first local run (60 seconds per target, about 5.4 million executions in
total) found no failures.
