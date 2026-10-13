# Token estimate calibration and overflow recovery — 2026-10-13

Branch `codex/token-calibration`. The runtime estimate (serialized bytes / 4 plus
16 per message) is now corrected per run by provider-reported input tokens, and a
context-length-exceeded error triggers one compaction and one retry. Behavior is
described in [context and attachments](../context-and-attachments.md#estimate-calibration-and-overflow-recovery).

## Automated checks (CI-class, scripted mock provider)

These are deterministic harness tests; the provider is scripted, not a model-quality claim.

- `crates/core/src/calibration.rs` unit tests: ratio update, smoothing, clamping to 0.5-4.0,
  ignored zero usage, tighten, raw/calibrated budget round trip, cached size equals the
  serialized definition.
- Property tests: classifier never panics and ignores non-400/413/422 statuses; JSON-like
  bodies without prose are never overflows; embedded markers are always detected; the ratio
  stays bounded for arbitrary observations; linear `context_split` equals the quadratic one.
- `crates/core/tests/runtime_e2e.rs` (wiremock): provider usage 3x the estimate compacts
  before the second request while the identical run without usage does not;
  overflow -> one compaction -> retry succeeds with the write executed once; two consecutive
  overflows fail explicitly with originals on disk and no tool executed.
- `crates/desktop/tests/token_calibration_e2e.rs`: shared app server plus `themis thread send`
  with an overflow on the first request; asserts the run finishes, three ordinary requests,
  one write, and the new optional `context_budget` fields.

## Live verification (separate from CI; real provider, free model)

Isolated `HOME` and data directory, throwaway git project containing copies of five docs
files (100 KB). Model `longcat-2.5-preview-free` through OpenCode Go; the key came from the
environment of the isolated server and was never printed.

```sh
cargo build -p themis-desktop
export HOME=$SCRATCH/live/home
themis --data-dir $SCRATCH/live/data server start
themis --data-dir $SCRATCH/live/data thread create $SCRATCH/live/proj longcat-2.5-preview-free
themis --data-dir $SCRATCH/live/data thread send $ID "Use read_file, one file per tool call, to read docs/configuration.md, docs/plugins.md, docs/ui-rules.md and docs/fuzzing.md (first page of each is enough). Then give a 5 bullet summary of what each file is about." --json
themis --data-dir $SCRATCH/live/data server stop
```

The run finished (19 tool calls, 9 model requests). Per-request values come from the
`token_calibration` records in `$SCRATCH/live/data/logs/diagnostics-0.jsonl` (metadata only).
"Calibrated" is the estimate used for that request, before its own report updated the ratio.
Errors are `(estimate - reported) / reported`.

| Request | Raw estimate | Calibrated estimate | Provider input | Raw error | Calibrated error | Ratio after |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 5840 | 5840 | 5484 | +6.5% | +6.5% (no observation yet) | 0.939 |
| 2 | 10132 | 9514 | 8981 | +12.8% | +5.9% | 0.913 |
| 3 | 13685 | 12491 | 11822 | +15.8% | +5.7% | 0.888 |
| 4 | 16368 | 14540 | 14061 | +16.4% | +3.4% | 0.874 |
| 5 | 18685 | 16325 | 15960 | +17.1% | +2.3% | 0.864 |
| 6 | 20998 | 18141 | 17739 | +18.4% | +2.3% | 0.854 |
| 7 | 23345 | 19945 | 19510 | +19.7% | +2.2% | 0.845 |
| 8 | 25292 | 21373 | 20939 | +20.8% | +2.1% | 0.836 |
| 9 | 25779 | 21563 | 21264 | +21.2% | +1.4% | 0.831 |

Result: after the first observation the calibrated estimate stayed within +5.9% of the
provider report (done criterion: within 15%). The raw estimate drifted from +12.8% to +21.2%
as tool output (JSON-heavy documentation text) accumulated, exceeding 15% from request 3.
The `context_budget` event carried `raw_estimated_tokens`, `provider_input_tokens` and
`calibration_ratio_milli` (for example 939 after request 1, 836 after request 8).

## Limitations

- One model, one conversation, one content type (English markdown). Other tokenizers
  (code, CJK, images) may drift differently; the ratio adapts per run but is not persisted.
- Here the byte estimate over-counted (ratio < 1), so calibration relaxes the budget.
  The tighter direction (ratio > 1) and overflow recovery are verified only with scripted
  providers; no live provider was driven into a real context overflow.
- The ratio is a single scalar over messages plus tool definitions; within a request the
  first observation cannot correct request 1.
- Smoothing is a fixed 50/50 average; sudden content shifts converge over a few requests.
- Providers that omit usage keep the raw estimate (unchanged behavior).
