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
| Turn endings | Any reply reads without a panic; the turn-end guard fires at most once between two tool calls; a repeated line is dropped however the stream splits it, and other text is shown whole | `crates/core/src/runtime/turn_end.rs` |
| Project paths | No mix of names, `.`/`..`, separators or symlinks resolves outside the project | `crates/desktop/src/state/workspace.rs`, `state/changes.rs` |
| Diffs | Arbitrary text never panics the parser; well-formed diffs round-trip; change detection reports exactly the differing files; readable files are NUL-free UTF-8 | `crates/desktop/src/diff.rs` |
| Recall | Arbitrary bytes never panic the record parser or bounded line reader; lines stay under the cap and reassemble; scan ranges stay in bounds | `crates/core/src/tools/recall/tests.rs` |
| Social messages | Any UI text never panics greeting detection or recall terms; terms bounded and sorted; social-only input is social (also repeated) with no terms; any non-social ASCII word makes it a task | `crates/core/src/social.rs` |
| Chat grants | Arbitrary text never panics the grant-file parser; anything it accepts is valid, bounded and never destructive, and never matches a destructive action | `crates/core/src/chat_grants.rs` |
| Sandbox | Seatbelt profiles never contain path text (paths travel as `-D` parameters); output scanning for denials never panics and yields bounded substrings; project layers never widen the sandbox; deny-rule flag matching only widens exact prefixes | `crates/core/src/sandbox/tests.rs`, `crates/core/src/configuration.rs` |
| Web pages and search | Any response body and content type never panics page extraction; any SearXNG body never panics, keeps only unique http(s) results within the requested count | `crates/core/src/tools/web.rs` |
| Browser | Any protocol message or accessibility tree never panics; snapshots stay within budget, refs stay stable and password values are masked; key names parse or are refused | `crates/core/src/browser/` |
| Model providers | Any `providers` block resolves or fails cleanly, never with a reserved id, literal key or non-https remote URL; any `/models` body keeps unique, usable ids that came from it | `crates/core/src/configuration/providers.rs`, `crates/core/src/providers/tests.rs` |
| Subagent profiles | Any text as a plugin's `agents/*.md` never panics; what it accepts is valid and bounded; a tool list never widens, and a scoped name is never mapped to the whole tool | `crates/core/src/plugins/agents.rs` |
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
| `recall_search` | Arbitrary bytes as a saved JSONL snapshot, plus query, source filter and `around` token for the `recall` tool |
| `provider_wire` | Model tool arguments and provider JSON for all response/stream converters |
| `chat_grants` | Arbitrary bytes as a chat's `chat_grants/<id>.json`: accepted entries are valid, bounded and never destructive |
| `mcp_sse` | Arbitrary bytes as an MCP server's event stream: reading it never panics, what it keeps stays bounded, and a message address it names stays on its server or is refused |
| `goal_verdict` | Arbitrary text as the goal check's reply (model output) and as a `/goal` message: never panics; a kept verdict is bounded, one line, free of control characters |
| `context_limit` | Arbitrary text as a provider's context-length error: reading the stated limit never panics, and a kept limit is a plausible window (8,000–1,000,000 tokens) |
| `sandbox_config` | User and project `runtime.jsonc` text plus command output: resolving never panics, a project never disables the sandbox, changes the backend, allows a blocked network or adds writable paths; no path reaches the Seatbelt profile text; denial scanning stays bounded |
| `social_request` | UI message text for greeting/thanks detection and recall terms: bounded output, a non-social word always makes a task |
| `patch_apply` | Model-written `apply_patch` hunks against file text: never panics, applied lines come from the file or the patch |
| `web_page` | Arbitrary bytes as a `fetch_url` response body under several content types and charsets |
| `browser_snapshot` | Arbitrary bytes as a browser protocol message and as an accessibility tree: never panics, the outline stays within its budget and a password field's value never appears |
| `searxng_results` | Arbitrary bytes as a SearXNG JSON response: results stay unique http(s) URLs within the requested count |
| `provider_models` | Arbitrary bytes as a configured provider's `/models` response and as the `providers` block of a user `runtime.jsonc`: never panics, kept ids are unique and usable, no resolved provider has a reserved id or a literal key |
| `subagent_profile` | Arbitrary bytes as a plugin's `agents/*.md`: never panics; an accepted profile has a safe name, bounded fields and only tools a subagent may have |
| `shell_wrapped` | Shell argv, often behind `sh -c`/`sudo`/`env`/`eval`/`cmd /c`: reading wrapped commands never panics, stays bounded and yields no empty commands |

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked
cd fuzz
cargo +nightly fuzz build --sanitizer none
cargo +nightly fuzz run --sanitizer none diff_parse -- -max_total_time=60
```

Targets run without AddressSanitizer: they cover safe Rust for panics and
invariant violations, and the sanitizer fails to link Tauri dependencies on
macOS. The `fuzz` workflow builds and runs each target for five minutes nightly (or on
demand), uploading `fuzz/artifacts` on failure. A pull request only builds the
targets when it changes `fuzz/` or the workflow, so a change that stops a target
compiling shows up in the next nightly run.

When a crash is found, minimise it (`cargo fuzz tmin`), fix the bug, and commit
the input as an ordinary regression test next to the code it exercises.

The first local run (60 seconds per target, about 5.4 million executions in
total) found no failures.
