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
retries with empty/transient failures. Each attempt has up to 300 seconds, under
one 900-second overall answer deadline including waits. The overall deadline may
shorten the final attempt.
Model, reasoning settings and output cap stay unchanged. Public text prevents
retry; failed-attempt tool calls never execute. Per-attempt metadata distinguishes
retries in context telemetry. Compaction stages already have automatic retries;
this change addresses ordinary generation before or after compaction.

Local results for commit `2d7b93b`: 182 core unit tests passed; 25 runtime integration tests
passed, with one opt-in live-provider test ignored. Workspace Clippy, formatting,
and 14 dashboard tests passed. Dedicated live forced-pressure acceptance remains
pending; an earlier natural 200k coding trial failed on ordinary provider output
truncation before any compaction, and its failure evidence was preserved.

The following live 200k coding trial confirmed retry starvation before compaction:
a truncated attempt took 169 seconds, then its retry was canceled after 130 seconds
because all attempts shared 300 seconds. The allocation regression reproduces this
with two 600 ms responses and a one-second attempt allowance; it failed before the
fix and passes with independent attempt allowances. Additional cases cover a
stalled attempt followed by success, public text followed by a stall with no retry,
persistent stalls bounded by the overall deadline, and Stop during a stalled
request. Fast persistent failures still allow exactly three attempts. No live
compaction acceptance is implied by these local timeout checks.

After the allocation fix, fresh checks passed: 183 core unit tests; 25 runtime
integration tests (one opt-in live test ignored); six configuration/diagnostic
checks; 13 shared-server/CLI checks; workspace Clippy and formatting; Ruff and
14 dashboard tests. The development desktop binaries rebuilt successfully.
No full unfiltered workspace/Keychain suite or fresh line coverage was run.

The next live 200k trial completed its partial coding phase and installed a real
checkpoint in 81.2 seconds, including one automatically recovered HTTP 429.
Its continuation then read a different project named in a global skill
description. The trial was stopped and is not accepted. Current project and
provider-session bindings were correct; this is not evidence of cross-session
provider leakage. Ordinary requests now include the captured current project
root in system context. Shared-server request assertions exercise that metadata
at initial execution, after forced compaction, and on a short follow-up. Live
path compliance still requires verification.

Further live acceptance uses one durable mixed-workload chat across 200k, 256k,
and the 24k stress transition. Coding, narrative, unrelated tasks, latest-task
follow-ups, older-task resumption and topic recall retain the same history.
Each budget requires a new successful checkpoint. Facts and requirements are
not resupplied at recall. Ordered phase records include budget, thread/run IDs,
latency, requests and tool calls. Warm document reuse is distinguished from
initial preparation, and evolving history prevents a clean efficiency comparison
across budgets without a matched successful baseline.
