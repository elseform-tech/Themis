# Forced compaction and bounded retry verification

The deterministic regression exercises the production checkpoint, evidence archive,
provider request, and tool execution paths at unchanged 24,000, 200,000, and
256,000-token budgets:

```sh
cargo test -p themis-core --test runtime_e2e forced_checkpoint_resumes_task_at_24k_200k_and_256k_without_replaying_writes
```

A partially completed ordered task is followed by distinct historical context
large enough to trigger automatic compaction. The test checks that original task
and completed-work state reach summary input, the installed checkpoint reaches
ordinary generation, original JSONL evidence remains readable, and useful work
finishes after the checkpoint. A truncated attempt proposes a duplicate write;
that proposal is discarded. The remaining write executes exactly once. Every
constructed provider request, including its output allowance and tool schema,
must fit its selected budget.

The provider is scripted. This is deterministic harness correctness proof, not a
claim that a live model preserves arbitrary facts. The separate live acceptance
case establishes a multi-file coding task and partial parser/revision edits,
applies distinct synthetic JSONL pressure through actual conversation turns,
requires a real checkpoint, then resumes with neither attachments nor repeated
requirements. It checks supplied tests, an independent oracle, immutable
reference tables, public APIs, staged sources, snapshots across every phase,
and provider usage availability. There is no imported history, artificially
lowered budget, or manual retry-compaction command in that live case.

Ordinary unpublished output-limit failures now share the existing maximum of two
retries with empty/transient failures, under one 300-second answer deadline.
Model, reasoning settings and output cap stay unchanged. Public text prevents
retry; failed-attempt tool calls never execute. Per-attempt metadata distinguishes
retries in context telemetry. Compaction stages already have automatic retries;
this change addresses ordinary generation before or after compaction.

Fresh local results: 182 core unit tests passed; 25 runtime integration tests
passed, with one opt-in live-provider test ignored. Workspace Clippy, formatting,
and 14 dashboard tests passed. Dedicated live forced-pressure acceptance remains
pending; an earlier natural 200k coding trial failed on ordinary provider output
truncation before any compaction, and its failure evidence was preserved.
