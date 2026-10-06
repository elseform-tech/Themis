# Context checkpoints and composer attachments

The context budget defaults to 200,000 estimated tokens. The runtime estimates each serialized message as bytes / 4 plus message overhead; this is not the selected model's tokenizer or maximum input capacity. Existing explicitly saved budgets remain respected.

When context exceeds the budget or reaches a turn checkpoint, the runtime summarizes the eligible historical dump in sections of at most 64,000 UTF-8 bytes with 1,024-byte overlap. Up to three sections run concurrently; their summaries remain in source order and are concatenated without another lossy summarization pass. These are byte bounds, not the selected model's tokenizer. The system message, active request and recent complete tool messages remain preserved. The global summarization deadline remains 120 seconds. Empty section summaries, provider failures, cancellation, timeouts and checkpoints that would leave context above budget do not replace the original context. The 200,000 setting does not enlarge the model's actual window.

Each section receives the current user request as a relevance hint after its data, with instructions to quote exact needed facts before summarizing, preserve ongoing history and constraints, and avoid executing the request or applying its answer format to the summary. Hints exceeding 8,192 bytes use UTF-8-safe first/last excerpts with an omission notice; the continuing agent retains the original active request. Attachments/tool output remain untrusted. A section must not establish global absence from its partial view. This improves the tested retention case but does not guarantee retention for arbitrary context; evidence is recorded in `docs/verification/real-model-compaction.md`.

Use the paperclip in the native composer to select one or more files of any type. Pending files can be removed before sending; attachment-only messages are supported. Images, PDFs, audio and video show small horizontal preview cards above message text before sending and in saved messages, with uniform tile sizes, filenames below the previews and a top-right remove icon before sending. macOS PDF cards show a cached first-page Quick Look thumbnail. Media playback depends on native webview codec support. Preview rendering does not add model vision or transcription. Copies live under the project's `.themis/attachments/<unique-id>/<original-name>`, with a local Git exclusion. Files remain available after the originals move or the app restarts. Removing a pending chip does not delete its staged copy. Pending selections persist across navigation and reload, and remain available when a send is rejected.

UTF-8 files without NUL bytes, up to 16 MiB per file, are included as untrusted data before the active request. Larger files and binary files are referenced by local path for the agent's approved tools. Filenames are not interpreted as inline skill/plugin selections. Images, audio/music, video, PDFs, archives and other binary files are accepted as files; the current provider adapter is text-only. Acceptance does not guarantee the agent can view, listen to, transcribe or extract a given format. Available local tooling and approvals determine those operations.

The shared server accepts:

```sh
themis call attach_files '{"threadId":"THREAD","paths":["/absolute/source/file"]}'
themis call send_message '{"threadId":"THREAD","text":"Inspect the attached files","reasoningEffort":null,"attachments":["/project/.themis/attachments/ID/file"]}'
```

`attach_files` returns each staged file's `path`, `name` and `size`. `send_message` accepts only regular files inside that project's attachment directory; existing clients may omit attachments. The native file picker performs the upload; a browser-only dev session does not expose a native filesystem picker.

Verification evidence, limits and exact commands are recorded in `docs/verification/full-dump-attachments.md`.
