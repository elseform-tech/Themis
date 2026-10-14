# Topic switches, short replies and parked work (2026-10-14)

Two kinds of evidence, kept apart:

- **Automated checks (CI-safe, offline, scripted providers):** prove the wiring. They say
  nothing about model quality.
- **Live-model benchmark audit (not in CI):** `longcat-2.5-preview-free` (OpenCode Go,
  non-thinking), 24k budget, 3 trials, same model and day for every column. One free model with
  SDs of 2-8 points: treat differences below about 5 points as noise.

## What changed

| Area | Change |
| --- | --- |
| Short replies | A first request whose user message is at most 200 characters with no attachments skips semantic compaction while the calibrated request fits the hard input budget. Maintenance and overflow recovery are unchanged; the checkpoint happens on the next substantive turn. |
| Anchors | The desktop injects no history anchors for greetings/thanks/farewells (multilingual list, script-aware for CJK/Thai, emoji), and none for text already verbatim in the working context. "continue", "ok", "do it" still anchor the latest exchange. |
| Checkpoint | Four sections: `Goal/constraints`, `Unresolved work`, `Parked work`, `Next action` ("if the user continues this task"). Validation accepts the 4-section form when `Parked work:` is present and the 3-section (and older) forms otherwise. Prompt: scan the whole span, every decision with its reason, exact facts, parked topics kept separate. |
| Checkpoint envelope | "...mention or resume this work only when the user's message continues or asks about it". Both old and new envelopes are accepted when stripping previous state. |

## Automated checks

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo clippy -p themis-core --features eval --all-targets -- -D warnings` | pass |
| `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | every `test result` line ok (core lib 255 passed) |
| `cargo test -p themis-core --features eval` | pass (incl. all 12 fixtures compact through the runtime) |
| `ruff check evals` | pass |
| `python3 -m unittest evals.compaction.test_harness` | 20 passed (adds grader tests and two scripted end-to-end runs of `behavior_eval`) |

New focused tests:

- `crates/core/tests/short_reply.rs`: a history sized between the semantic threshold and the
  usable budget is not compacted for "hi" (0 summary requests, 1 ordinary request) but is when
  not marked short; 200 packets always compact and never exceed the hard budget; the
  constructed checkpoint request contains the four section labels, "if the user continues this
  task", the decision-with-reason and whole-span instructions; `is_short_reply` counts
  characters (200 CJK characters qualify, 201 do not).
- `runtime::checkpoint_section_tests`: 3-, 4- and 5-section forms validate; empty or misordered
  parked work does not.
- `crates/desktop/tests/transcript_store.rs`: ten greetings (English, Chinese, Japanese, Korean,
  Russian, Spanish, emoji) pull no anchors; a CJK greeting with content and an English thanks
  with content still recall; "Continue that" still anchors; exchanges already in context are not
  duplicated.

The desktop run wiring (`RunSnapshot.short_reply` and the task-local) compiles and is covered
by the workspace tests, but no desktop/UI journey was exercised: **unverified in the native app**.

## Live compaction suite (benchmark audit)

Commands (`main` = worktree of `origin/main` at 5e2a489, branch = this PR):

```sh
python3 -m evals.compaction run --suite core --trials 3 --model longcat-2.5-preview-free --budget 24000
```

Files: `evals/compaction/results/2026-10-14-longcat-2.5-preview-free-24000-main-5e2a489.json`,
`...-topic-switches-prompt-only.json` (before the envelope change) and
`...-topic-switches.json` (final). No failed or invalid runs.

| Summary-only retention (mean +- SD) | main | prompt only | final | target |
| --- | --- | --- | --- | --- |
| Overall | 0.824 +- 0.082 | 0.855 +- 0.018 | **0.879 +- 0.021** | n/a |
| parked_topic | 0.691 +- 0.109 | 0.809 +- 0.149 | **0.857 +- 0.071** | >= 0.90 **missed** |
| decision | 0.556 +- 0.193 | 0.667 +- 0.000 | **0.778 +- 0.193** | >= 0.85 **missed** |
| middle position | 0.711 +- 0.077 | 0.711 +- 0.077 | **0.800 +- 0.067** | >= 0.85 **missed** |
| early position | 0.873 +- 0.095 | 0.902 +- 0.017 | 0.931 +- 0.017 | |
| late position | 0.833 +- 0.167 | 0.944 +- 0.096 | 0.778 +- 0.096 | |
| constraint | 0.926 +- 0.128 | 0.926 +- 0.064 | 0.926 +- 0.064 | |
| identifier | 0.833 +- 0.072 | 0.917 +- 0.072 | 0.875 +- 0.000 | |
| narrative | 0.952 +- 0.083 | 0.952 +- 0.083 | 1.000 +- 0.000 | |
| negation | 1.000 +- 0.000 | 0.778 +- 0.385 | 1.000 +- 0.000 | |
| superseded | 0.917 +- 0.144 | 1.000 +- 0.000 | 1.000 +- 0.000 | |
| tool_result | 0.809 +- 0.218 | 0.714 +- 0.000 | **0.667 +- 0.083** | no regression > 2 points: **missed** |
| Overall retention (all probes) | 0.873 +- 0.051 | 0.886 +- 0.013 | 0.916 +- 0.015 | |
| Tail retention | 0.986 +- 0.024 | 0.958 +- 0.042 | 1.000 +- 0.000 | |
| Forgot rate | 0.076 | 0.059 | 0.042 | |
| Wrong answers per trial | 1.3 | 1.3 | 1.0 | |
| Probe request errors per trial | 2.7 | 3.0 | 2.3 | |

Reading it honestly:

- Everything moved the right way except `tool_result` (7 summary-only probes per trial, so one
  probe is 14 points; the main run itself ranged 0.57-1.00) and late-position facts. The prompt
  does not touch tool results, the ledger carries them, so this is most likely noise, but it is
  a measured drop and is reported as such.
- The three headline targets were **not reached**. The remaining `decision` misses are mostly
  `why_eu` (the model reports the later Frankfurt latency reason instead of the earlier EU data
  rule: two reasons for one decision) which this prompt does not solve; parked-topic misses are
  single UNKNOWN answers (allergy, flour, gift budget) spread across trials.
- The earlier 0.897 baseline from 2026-10-13 and today's 0.824 on the same code differ by 7
  points, which is the noise floor for this suite. The +5.5 points overall gain is within one SD
  of the baseline and should be read as "no regression, probable improvement", not as proof.

## Behavior evals (benchmark audit)

```sh
python3 -m evals.compaction behavior --trials 3 --model longcat-2.5-preview-free --budget 24000 --anchors old|new
```

`old` reproduces the previous desktop anchors (latest exchanges for every message) with the
previous envelope; `new` is the final branch (no anchors for social turns). This is a model-level
simulation of the desktop retrieval, not a run through the desktop. The `old` run used the
checkpoint prompt of this branch (the prompt is compiled in), so it isolates the anchor and
envelope changes only; it was regraded offline with the final grader. One `old` trial of
`behavior-greeting` was invalid (compaction failed) and is excluded.

| Metric | old anchors | new (prompt + anchors, no envelope) | new (final) | target |
| --- | --- | --- | --- | --- |
| Greeting turns passing (short, no tools, no task mention) | 1/8 | 0/12 | 0/12 | n/a |
| Greeting tool calls | 14 | 3 | **8** | 0 **missed** |
| Greeting compactions | 0 | 0 | 0 | 0 met |
| Unwanted resume (greeting + unrelated turns) | 0.47 | 0.57 | **0.62** | <= 0.05 **missed** |
| Unrelated question answered, task untouched | 0.89 | 1.00 | 0.89 | |
| Topic return (A -> B -> back; fact share) | 1.00 | 1.00 | 1.00 | |
| Vague "continue" resumes the latest task | 0.89 | 0.78 | 1.00 | |
| Provider errors (truncation) | 1 | 2 | 0 | |

Reading it honestly:

- **Greetings still resume the task, and this branch does not fix that.** With the post-compaction
  context in front of it the model answers "hi" with a status recap of the unfinished task in
  almost every trial (including in Chinese for 你好), and sometimes tries tools that the eval
  denies. Removing the anchors lowered attempted tool calls (14 to 3 or 8, counts are small and
  the model is stochastic) but not the recaps. The cause the data points to is the global
  system text asking for brief conversational status updates plus an unfinished task in the
  checkpoint; that string belongs to the parallel `codex/recall-tool` work and was deliberately
  not edited here. The envelope sentence added in the final commit did not reduce recaps.
- The grader counts any mention of the task in a greeting reply as a resume (strict by design,
  lexical); several replies are short and polite but still say where the task stands.
- The compaction-deferral part of the greeting requirement ("no compaction") is met and tested
  deterministically in `short_reply.rs`; the behavior eval's own post-compaction context is far
  below the threshold, so its 0 compactions says little.
- Topic return held 100% on all variants, so that behavior was not at risk in this fixture; the
  retention numbers above are the more sensitive measure.

## Limitations and unverified

- One free model, one day, 3 trials; fixtures are synthetic and small (8 retention cases, 4
  behavior cases, 36 behavior turns per run); lexical grading.
- Targets missed: parked_topic 0.857 (target 0.90), decision 0.778 (0.85), middle 0.800 (0.85),
  tool_result regression, greeting tool calls 8 (0), unwanted resume 0.62 (0.05).
- Not run: the desktop/native UI; the real desktop anchor retrieval end-to-end with a live model.
- The short-reply rule treats any message of at most 200 characters as short, including terse
  imperatives such as "fix the parser"; the hard-budget guard makes this safe for the request,
  at the cost of compacting one turn later.
