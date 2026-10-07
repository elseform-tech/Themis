# Recoverable compaction verification — active goal

Status: implementation and verification ongoing; do not claim completion or lossless summary retention.

Goal: compact working state while durably preserving original conversation/tool/source evidence and validating retrieval after repeated real-model compactions and restart.

Implementation remains on `codex/full-dump-attachments`, continuing the existing compaction task. Production budget200000, global checkpoint deadline150s; section input64000UTF-8bytes, overlap1024, concurrency3. New summaries aim for300words; evidence references are deterministic. Source-complete recovery checkpoints explicitly mark unavailable summaries. Original-source availability and summary-only retention must be measured separately.

Acceptance: fresh workspace/CLI checks, fault/cancellation/budget/storage checks, repeated narrative runs using the unchanged six source files and 24 preregistered criteria, recovery-assisted source checks after production server restart, and a coding task with large repository/tool context, source-version recovery, requirements/unrelated changes preserved, and fresh executable tests. Report all failures and limitations.

Preliminary production build SHA2564268e296ec65ba3e59fc5da33b3aeab950e5343260d6716e7ba9f190ff974243 (before storage/fallback fixes). Thread55a3beb8-1f75-4e8c-a975-c463c816d081: part1 checkpoint35278bytes, finished~90s; part2 checkpoint41848bytes, finished~150s (includes continuing answer); part3 timed out at150s. No final retelling or24-fact score. Saved originals for all three parts remain available. No semantic reliability claim follows from this partial run.

Local artifacts: `/tmp/themis-narrative-six/run-evidence.py`, `run-evidence.log`, `thread-evidence.json`, `evidence-recoverable.json`; `/tmp/themis-evidence-server.log`. Earlier fixtures/hashes/rubric unchanged under `/tmp/themis-narrative-six/`. Unit red/green logs `/tmp/themis-evidence-*.log`. Final production retest and coding journey remain pending.
