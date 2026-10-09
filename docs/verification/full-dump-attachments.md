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
| `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | 276 passed, 0 failed; 2 ignored integration cases; 1 filtered Keychain case |
| `cargo test -p themis-core --lib` | 92 passed |
| `cargo test -p themis-desktop --test attachments_e2e` | 2 passed |
| `npm --prefix web run lint` | Passed |
| `npm --prefix web test` | 27 files, 283 passed |
| `npm --prefix web run build` | Passed; existing >500 kB chunk warning |
| `python3 -m unittest quality_dashboard.test_dashboard` | 13 passed |
| `ruff check quality_dashboard` | Passed |

Rust ignores include the network public-repository acceptance and live-provider check. The ordinary workspace run also ignores the documentation example. The filtered synthetic macOS Keychain write test is the known host-specific failure; no claim is made that an unfiltered suite passes. Vitest reports its existing environmentMatchGlobs deprecation.

The native build used the cached Tauri CLI from `crates/desktop`, with `tauri build --bundles app --config /tmp/themis-attachments-qa-config.json`; the override disabled the redundant pre-build command and isolated app identity. The frontend production build was run separately.

## Code health

Refreshed dashboard metrics with rust-code-analysis 0.0.25, excluding the temporary native helper. Product scope includes Rust inline test code. Available Rust line coverage is 13,737/16,011 (**85.8%**), about 0.2 percentage points below the earlier 86.0% snapshot. No frontend line coverage was generated.

| Module | Mean cyclomatic | Mean cognitive | Maintainability index | Rust lines covered |
| --- | ---: | ---: | ---: | ---: |
| core runtime (including inline tests) | 2.13 | 0.99 | 67.3 | 850/918 |
| attachment storage/context | 2.52 | 0.86 | 81.8 | See generated LCOV |
| desktop threads | 2.02 | 0.89 | 78.1 | 411/488 |
| composer | 3.17 | 3.20 | 72.6 | Not measured |

Runtime complexity improved from 2.23 cyclomatic / 1.12 cognitive; its maintainability index decreased from 68.1 to 67.3 with added regression code in the same file. These are AST metrics, not behavioral guarantees. The dashboard snapshot includes the final ThreadView synchronization fix. Overall: 103 files analyzed, mean cyclomatic 2.49, cognitive 2.21, maintainability 73.8. The native bridge (including inline regression) measures 2.24 / 0.94 / 74.9; the preview component 2.00 / 2.62 / 82.5. The generic-file icon and explicit accessibility label increased preview cognitive complexity from 2.10 to 2.62; the component remains 26 lines.

## Limits

The budget defaults to 200,000 estimated tokens; existing explicit settings remain respected. Full eligible history is summarized in one request, without chunking fallback. An input beyond the provider's actual capacity can still fail, and a large active user request that cannot fit after checkpointing remains an error rather than being silently truncated.

All regular file types can be staged. UTF-8 files without NUL bytes, up to 16 MiB each, enter context. Binary and larger files remain accessible by path to the existing shell/Bash and other approved tools. Automatic transcription, media vision and document extraction are not implemented; installed commands determine what the agent can decode. Removing a pending chip leaves the staged copy on disk.

## Preview follow-up

Attachments use equally sized 136×126-pixel horizontal cards above the composer text and above sent message text. Filenames appear below previews; pending cards have a remove icon in the top-right corner. There is no Open action. Structured attachment paths survive saved history. Image thumbnails, audio/video controls and PDF previews share the card. Files without an inline renderer show a file icon and filename. Native asset scope starts empty; only exact canonical files approved through the shared project attachment validator are allowed.

The initial PDF iframe was blank in WKWebView. macOS now uses Quick Look to cache a first-page PNG beside the staged PDF. The cache is per staged copy and does not refresh after in-place edits. On other platforms the sandboxed PDF iframe remains the fallback; that rendering is unverified.

Earlier native checks confirmed the synthetic PDF's visible page content in the composer and the full one-page document in macOS Preview before the Open action was removed. Sending only that PDF through the controlled provider produced `PDF attachment received.` and preserved the page preview in the user message while clearing the composer selection. This checks file transport/display, not model understanding of the PDF. Image pixels also rendered. Audio/video element wiring is covered automatically; video playback and audible output remain unverified.

After rebuilding and restarting the final native layout, the saved PDF preview restored above `Verify PDF attachment`. Selecting PDF, image and audio together produced three small cards in one horizontal row above the composer draft `Preview layout`; PDF and image pixels and the audio Play control were visible.

The new macOS `pdf_thumbnail_renders_and_rejects_escaped_cache` regression exercises actual Quick Look output, verifies the PNG signature/cache reuse and rejects an external cache symlink. CLI tests verify the preview path and reject escaping paths. Frontend tests verify restored PDF cards and attachment-before-message DOM order. The server contract test now includes calls originating in the native Tauri bridge.

Intermediate checks caught a component-layer contract violation, a missing native command in the contract inventory, and a `/var` versus `/private/var` cache path mismatch. They were fixed; the fresh full runs listed above passed afterward.

Final review replaced `exists`/`create_dir` with idempotent `create_dir_all` so simultaneous previews cannot fail on an already-created cache directory. After this change, `cargo test -p themis-desktop --lib pdf_thumbnail_renders_and_rejects_escaped_cache` passed (1 test), formatting and workspace Clippy passed, and AST metrics were refreshed. The full coverage suite was rerun after this directory-creation change and removal of the unused native Open branch.

The final native build restored the saved PDF card above message text without an Open action. PDF, image and ten-second silent WAV controls rendered in equally sized cards, with filenames below and remove icons inside the top-right corner. Clicking the image remove icon removed only that card and retained the PDF, audio and draft. Clicking audio Play changed it to Pause; pausing restored Play. This observes playback state, not audible output.
