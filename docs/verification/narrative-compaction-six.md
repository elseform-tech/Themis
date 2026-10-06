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
