# Six-compaction narrative test: failed before completion

Verified 2026-10-07 Europe/Berlin against unchanged implementation commit `acc126e` (runtime change `3f7a9e5`).

**The requested six-compaction narrative journey did not complete.** Two checkpoints succeeded, but the third part hit the 120-second summarization timeout twice. There is no final complete-story retelling and no six-compaction semantic retention score. This richer test does not support claiming that the earlier sparse-record result generalizes to long narratives.

## Real input and execution

Used the production `target/release/themis --data-dir /Users/cutedandelion/Library/Application Support/ai.themis.desktop call ...` interface against the same shared backend used by the production desktop app. Provider/model: OpenCode Go / `muse-spark-1.3-contributor`. Saved context budget: **200000**, rechecked after the runs. No mock provider, debug build, product change or timeout adjustment was used. Native composer/upload rendering was not repeated; the native app was used to deny unrelated browser-use MCP startup on each send. There were no model recovery tool calls or file rereads.

The input was a continuous real narrative from Samuel Richardson's *Clarissa*, using Project Gutenberg volumes 1–9, with publishing headers, contents and the final author postscript removed. Line endings were normalized. Characters were renamed consistently (for example Clarissa → Celia, Harlowe → Fernbrook, Lovelace → Valebrook, Belford → Sayer, Solmes → Fenwick) to reduce reliance on memorized names; original nicknames and the recognizable plot remain, so pretraining knowledge is not eliminated. The narrative was divided into six nonoverlapping consecutive parts at paragraph boundaries. There were no marker records or repeated artificial filler. A cut can occur within a letter.

Sources: [volume 1](https://www.gutenberg.org/ebooks/9296), [volume 2](https://www.gutenberg.org/ebooks/9798), [volume 3](https://www.gutenberg.org/ebooks/9881), [volume 4](https://www.gutenberg.org/ebooks/10462), [volume 5](https://www.gutenberg.org/ebooks/10799), [volume 6](https://www.gutenberg.org/ebooks/11364), [volume 7](https://www.gutenberg.org/ebooks/11889), [volume 8](https://www.gutenberg.org/ebooks/12180), [volume 9](https://www.gutenberg.org/ebooks/12398).

| Part | UTF-8 bytes | cl100k_base / o200k_base tokens | SHA-256 |
| --- | --- | --- | --- |
| 1 | 839,810 | 209,897 / 208,662 | `5cc4c0af76213d56f5bf82f866d9654f9cbc1876d60d40e5a50a35d03a0d663b` |
| 2 | 839,694 | 211,012 / 209,827 | `b9f6f3eacb3f2f4cf1c8724d1d0c279bcf2acfc5cc598ea91e5bb8dcba2912e0` |
| 3 | 839,614 | 212,924 / 211,842 | `f08e8c1360c9ccc21466d907e03e1f52e92f00f0670a26f94e1f0a0d6dcbc779` |
| 4 | 845,282 | 214,895 / 213,846 | `52eec719d240bad9abcec6a7c4a13e53efe058bf5e90fc34d75ff8b8e02de862` |
| 5 | 838,917 | 214,162 / 212,683 | `ecb7cd97bfc8c64f5cfa858f2b6ab903c140ce13e120cff5ce08ce89ae604bfd` |
| 6 | 839,491 | 212,685 / 211,463 | `d599345b2bdd28f9f8ee40388a3a3afc9c50db4750e224c4d125ef79d4db3afd` |

These are reference tokenizers, not a measurement of Muse's tokenizer. All six parts were prepared, but only the first three were submitted. Actual staged copies matched the source hashes for every submitted attempt. The app's persisted `context_compacting` events confirm the submitted parts forced compaction. Only new prose was attached each turn; previous story content had to come through the saved checkpoint. Failed parts were retried with byte-identical input, in the same conversation.

Every request asked to preserve character relationships and changing motives, chronology, promises/deceptions, events versus allegations, and unresolved threads/resolutions, using only provided conversation context without tools or rereads. Turns 1–5 were to answer only `RECEIVED`, preventing earlier assistant retellings from serving as extra copies. Turn 6 was to produce a faithful 1200–1800-word chronological retelling, character final-state table, and explicit uncertainties. The sixth request was never reached.

## Persisted outcomes

Thread: `67bbdf8b-a1b7-454e-ad2b-6d7e525819d0`.

| Part / attempt | Run | Sequences | Result | Checkpoint UTF-8 bytes | Observed total after send returned |
| --- | --- | --- | --- | --- | --- |
| 1 | `4dc8b9bf-065e-44a7-9b89-89adfbf1bbcd` | 433–439 | Completed, RECEIVED | 70,160 | ~100 s |
| 2, first | `5de385d7-db96-4f8d-9cd5-5bf2ccb6844a` | 440–444 | Summary timeout | None | ~120 s |
| 2, retry | `d89b12a9-8aac-4135-86d0-49eeb7e9cf21` | 445–451 | Completed, RECEIVED | 80,125 | ~150 s |
| 3, first | `1dc17b9a-a4e4-4d86-88d7-150684df0ee9` | 452–456 | Summary timeout | None | ~120 s |
| 3, retry | `48fae562-d184-4ef5-9d92-71a6c4a40347` | 457–461 | Summary timeout | None | ~120 s |

All failed terminal events contain `Context checkpoint failed: summarization timed out before completing checkpoint`. Times use five-second polling and include the continuing answer after a successful checkpoint; the 150-second retry does not mean the 120-second compaction timeout changed. Exact per-section and summary latency were not instrumented.

The final stored checkpoint remains through sequence **449**, byte-identical to the second successful checkpoint. Both third-part failures left that checkpoint intact. The thread was independently checked as `running: false`. This verifies safe failure, not retention of the uncheckpointed third part. The attachments remain staged; no silent partial checkpoint was accepted.

Spot checks of the second checkpoint found the earlier inheritance/family jealousy, Fenwick marriage pressure, prior Rosamund courtship and subsequent departure material. These mentions are preliminary evidence, not a full semantic fidelity score. Twenty-four source-anchored narrative checks were registered before any final answer; they were not sent to the tested model. Because the complete-story run failed, they remain **unscored**:

- inheritance: Grandfather favors Celia; siblings become jealous.
- prior_courtship: Valebrook first courts Rosamund, then Celia.
- forced_match: Family pressures Celia to accept Fenwick; she rejects him.
- father_curse: Her father curses her after flight.
- departure: Celia leaves under pressure/deception, not a simple voluntary romantic elopement.
- false_house: Rooke's respectable house conceals sexual exploitation.
- servant: Dorcas participates in deception.
- false_mediator: Tomlinson falsely claims family connection/reconciliation.
- false_relatives: Impersonated relatives deceive Celia.
- fire: A fire alarm assists the plot against Celia.
- assault: Valebrook assaults Celia after potions impair her senses, without consent.
- refusal: Celia rejects marrying Valebrook after the assault.
- penknife: Celia threatens her life with a penknife to stop another attack.
- honest_lodging: The Briars provide honest refuge and sell/make gloves.
- debt_arrest: Celia is arrested for lodging debt through Rooke's circle.
- executor: Sayer becomes protector/executor despite being Valebrook's friend.
- death: Celia dies September 7 without marrying Valebrook.
- forgiveness: She forgives family despite rejection, reconciliation comes too late.
- funeral: Body returns home; family mourns/regrets.
- anti_revenge: She urges Ashcombe to allow Valebrook repentance rather than revenge.
- fatal_duel: Ashcombe duels Valebrook near Trent; Valebrook dies.
- friend_marriage: Ada marries Tolland after six months' mourning.
- sayer_reform: Sayer reforms and later marries Charlotte Montague.
- brother_aftermath: James marries disputed estates and mourns annually from Sept 7.

## Interpretation and next step

The bounded approach that passed sparse-record tests is not yet reliable for this dense narrative workload under the current deadline. This experiment identifies a timeout/reliability failure before a six-compaction quality judgment is possible. Rich prose generated larger checkpoints (70–80 KB versus roughly 40–50 KB in the earlier tests), and subsequent checkpoints add more work; that is a plausible contributor, not a measured attribution of the provider's internal latency. No per-section timing or provider-usage capture was made here, so individual slow sections, remote load and caching effects remain unseparated.

To finish a meaningful six-round retention evaluation, first measure section latencies and output sizes, give the total operation a deadline appropriate to the actual workload while retaining per-request timeouts/cancellation, and reduce repeated instructions/overlong extracts in section summaries. Keep the context budget at 200000 and rerun the same source before claiming improvement. Merely increasing the deadline may enable the experiment; it does not establish semantic retention or guarantee bounded checkpoint growth. No such implementation change was made during this verification.

Local artifacts: `/tmp/themis-narrative-six/manifest.json`, `rubric.json`, `evidence.json`, `run.log`, `retry.log`, `retry3.log`, and the preparation/orchestration scripts in that directory. The authoritative events and latest checkpoint are in the production SQLite store. Full novel text and credentials were not committed.

Fresh checks: `python3 -m unittest quality_dashboard.test_dashboard` passed 13 tests; `ruff check quality_dashboard` passed. A final scoped assertion checked staged-source hash equality and unchanged last checkpoint after the failed attempts. `git diff --check` passed. Product source/metrics/coverage are unchanged from the preceding verification; full Rust and frontend suites were not rerun for this documentation-only follow-up.


## Retest with 150-second compaction timeout

On 2026-10-07 the user authorized an additional 30 seconds. Commit `16f1be2` changes only the shared runtime timeout constant from 120 to **150 seconds**, plus the current behavior documentation. The saved context budget remains **200000**. The production app was rebuilt; all known project threads were checked idle, and both the production window and shared server were restarted to load the changed runtime. The running server executable was verified as the new production bundle. Executable SHA-256: `5c8a1da89b021c9e6ca4d9c1b4e0e573510bbd23ebc8f0bc587b1bb0e1652680`.

**The retest still failed to complete six compactions.** Five parts completed on their first attempt; the final part timed out at 150 seconds on both its initial attempt and one byte-identical retry. No final complete-story answer exists, so the 24 narrative retention checks remain unscored. No additional timeout increase or prompt change was made.

A fresh conversation used the same six files and prompts above, through the production CLI/shared server with the same real model. Input hashes were verified against every staged attachment. The first five answers were exactly `RECEIVED`; none repeated narrative content. Unrelated browser-use MCP startups for parts 2–6 and the final retry were denied in the native app. There were no model recovery calls or attachment rereads. The first part had no startup approval event in this rerun.

Thread: `efdf0b9e-ff48-4f00-bd1d-748dd1bd35b5`.

| Part / attempt | Run | Sequences | Outcome | Checkpoint UTF-8 bytes | Observed total after send returned |
| --- | --- | --- | --- | --- | --- |
| 1 | `8c975d6e-5d3c-4575-9048-b66a1f7dc996` | 462–467 | Completed | 61,273 | ~115 s |
| 2 | `6f1595cb-32a2-4ad1-a25a-4886ac3a9665` | 468–474 | Completed | 62,071 | ~115 s |
| 3 | `b6bea07f-2b4c-4ff7-98c9-f5e6bd54d0e7` | 475–481 | Completed | 62,052 | ~110 s |
| 4 | `4b1e78d2-7bb0-4637-96f2-dc1b67fc318c` | 482–488 | Completed | 70,164 | ~90 s |
| 5 | `8664768d-b76d-47bc-9672-9c5d1afaec66` | 489–495 | Completed | 88,439 | ~130 s |
| 6, first | `d22d5f38-0cfa-44ba-a77f-a7144514ecb2` | 496–500 | Summary timeout | None | ~150 s |
| 6, retry | `9459803e-acad-46bd-9b45-d429f626b32f` | 501–505 | Summary timeout | None | ~150 s |

Times include any continuing answer and use five-second polling; they are not exact summary-stage or per-section timings. Both failures report `Context checkpoint failed: summarization timed out before completing checkpoint`. The stored checkpoint remains through sequence **493**, byte-identical to the fifth successful checkpoint. The conversation is no longer running. A scoped assertion verified the five checkpoints, two timeout events, source/staged hash matches and unchanged last checkpoint. This confirms safe failure while leaving six-round narrative effectiveness unverified.

This rerun progressed farther than the 120-second experiment, but it is not a controlled causal comparison of deadline alone: model/provider variability and input caching were not measured, and earlier checkpoints differ between runs. Successful parts whose total runtime was below 120 seconds do not prove the additional allowance caused success. The repeated final failure does demonstrate that a fixed 150-second global deadline is still insufficient for this tested workload. Before increasing it again, measure section timings and output sizes to distinguish slow requests from accumulated work.

Fresh verification:

```sh
cargo test -p themis-core --lib compaction
cargo test -p themis-desktop --test attachments_e2e cli_large_file_upload_compacts_once_and_preserves_binary_files -- --nocapture
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
python3 -m unittest quality_dashboard.test_dashboard
ruff check quality_dashboard
```

Core compaction tests: **2 passed** (bounded large-input delivery and timeout preserving original conversation). Isolated CLI attachment E2E: **1 passed**, 1 filtered, 2.68 s. Format and Clippy passed. Dashboard: **13 passed**; Ruff passed. Production release build passed in 1m 01s using the previously documented build command/override. Full Rust and frontend suites were not rerun for the timeout-constant change.

The dashboard was started at `http://127.0.0.1:4178` and `/api/metrics` refreshed. Product metrics remain 103 files, mean cyclomatic 2.49, cognitive 2.22, maintainability 73.8. Runtime including inline tests remains 1,309 LOC, 46 functions, cyclomatic 2.19, cognitive 1.14, maintainability 67.3. Existing Rust coverage is 13,825/16,104 (85.8%); coverage was not regenerated and frontend coverage remains unavailable. No metric regression at reported precision.

Local evidence: `/tmp/themis-narrative-six/evidence-150.json`, `run-150.log`, `retry6-150.log`, `metrics-150.json`, and `build-150.log`. The production SQLite store remains authoritative. The 150-second timeout is left installed as requested; the context budget is unchanged.
