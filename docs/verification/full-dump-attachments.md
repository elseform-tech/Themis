# Full-dump compaction and attachments verification

Verified 2026-10-06 on `codex/full-dump-attachments`.

## Control and request path

The control file contains `BEGIN-17`, 105,001 repetitions of ` apple`, `MIDDLE-42`, another 105,001 repetitions, then `END-99`: 1,260,035 bytes. An isolated temporary tiktoken environment counted **210,012 tokens** with both `cl100k_base` and `o200k_base`; Themis estimates approximately 315,009 tokens before message overhead. The selected production model's tokenizer was not verified.

`cli_large_file_upload_compacts_once_and_preserves_binary_files` drives the actual `themis call attach_files` and `themis call send_message` commands against an isolated shared app server and synthetic HTTP provider. It verifies one complete summary request, one answer request, all three markers, final completion, persisted attachment references, unchanged copied bytes, and Git exclusions. Binary fixtures cover image, music, video, archive and a capability-shaped filename. These are byte-transport fixtures, not media-decoding tests.

Core regressions verify a single whole-dump summary request and preservation of the original context if the replacement would exceed budget. Existing timeout/cancellation checks passed. The deadline remains 120 seconds; changing to one request removes repeated sequential calls, but does not establish that a real provider can summarize every large input within that deadline.

## Native composer

Built an isolated `ThemisAttachmentsQA.app` with identifier `ai.themis.attachmentsqa`. A temporary helper supplied the same app server with an in-memory synthetic credential and HTTP provider; it was removed from the repository afterward. No production credential was used. The installed production app was not replaced.

Through the native UI: opened the seeded chat, clicked Attach files, selected the control in the macOS picker, observed its chip and 1,260,035-byte size, entered `Report all three markers`, and clicked Send. The visible answer was `Control verified: BEGIN-17, MIDDLE-42, END-99.`; the pending chip cleared. The captured summary HTTP body was 1,261,118 bytes and the answer body 25,012 bytes, both with all control markers. This verifies the native composer, bridge, shared server, runtime checkpoint and answer display. It does not verify live-model summary quality or input capacity.

The subsequent navigation-race fix was verified with the frontend regression: leave and return before upload completion; leave and return before send completion; attach another file; verify only submitted files clear. This exact race was not reproduced manually in the native app.

## Fresh checks

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | Passed before final frontend race fix |
| `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | 275 passed, 0 failed; 2 ignored integration cases; 1 filtered Keychain case |
| `cargo test -p themis-core --lib` | 92 passed |
| `cargo test -p themis-desktop --test attachments_e2e` | 2 passed |
| `npm --prefix web run lint` | Passed |
| `npm --prefix web test` | 26 files, 281 passed |
| `npm --prefix web run build` | Passed; existing >500 kB chunk warning |
| `python3 -m unittest quality_dashboard.test_dashboard` | 13 passed |
| `ruff check quality_dashboard` | Passed |

Rust ignores include the network public-repository acceptance and live-provider check. The ordinary workspace run also ignores the documentation example. The filtered synthetic macOS Keychain write test is the known host-specific failure; no claim is made that an unfiltered suite passes. Vitest reports its existing environmentMatchGlobs deprecation.

The native build used the cached Tauri CLI from `crates/desktop`, with `tauri build --bundles app --config /tmp/themis-attachments-qa-config.json`; the override disabled the redundant pre-build command and isolated app identity. The frontend production build was run separately.

## Code health

Refreshed dashboard metrics with rust-code-analysis 0.0.25, excluding the temporary native helper. Product scope includes Rust inline test code. Available Rust line coverage is 13,661/15,897 (**85.9%**), about 0.1 percentage points below the earlier 86.0% snapshot. No frontend line coverage was generated.

| Module | Mean cyclomatic | Mean cognitive | Maintainability index | Rust lines covered |
| --- | ---: | ---: | ---: | ---: |
| core runtime (including inline tests) | 2.13 | 0.99 | 67.3 | 850/918 |
| attachment storage/context | 2.54 | 1.04 | 83.1 | 128/159 |
| desktop threads | 2.02 | 0.89 | 78.1 | 411/488 |
| composer | 3.17 | 3.20 | 72.6 | Not measured |

Runtime complexity improved from 2.23 cyclomatic / 1.12 cognitive; its maintainability index decreased from 68.1 to 67.3 with added regression code in the same file. These are AST metrics, not behavioral guarantees. The dashboard snapshot includes the final ThreadView synchronization fix.

## Limits

The budget defaults to 200,000 estimated tokens; existing explicit settings remain respected. Full eligible history is summarized in one request, without chunking fallback. An input beyond the provider's actual capacity can still fail, and a large active user request that cannot fit after checkpointing remains an error rather than being silently truncated.

All regular file types can be staged. UTF-8 files without NUL bytes, up to 16 MiB each, enter context. Binary and larger files remain accessible by path to the existing shell/Bash and other approved tools. Automatic transcription, media vision and document extraction are not implemented; installed commands determine what the agent can decode. Removing a pending chip leaves the staged copy on disk.
