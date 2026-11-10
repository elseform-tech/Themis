# Agent weaknesses

What the agent has been seen doing wrong in real chats (live checks, the tour, eval traces, a
person's report), and the eval that measures each one. The point is to know what to expect from a
model and to see whether a change helps: every entry names a measure that an eval run reports,
and its latest result. Harness and UI bugs are not listed here; they get regression tests.

## How to add one

1. Describe what the agent did, where and when, in a line, with the model.
2. Make it measurable in the suite that fits, with a grader test built from the text or files seen:
   - a store, tool or wording mistake around plugins, skills, MCP or hooks: `evals/integrations`
     (a scenario with a hidden check, or a measure on every run);
   - a coding task's correctness, verification habits, false claims: `evals/verify`;
   - long goals, checkpoints, memory, false completion: `evals/soak`;
   - anything else: the nearest suite under `evals/`, or a new scenario there.
3. Record the measure and the latest result below; update it whenever the eval runs again.
4. A change meant to fix it (a prompt, a guide, a tool) is A/B tested against that measure before
   it merges, and the entry says what moved.

Status: **open** (seen, measured, not improved yet), **improving** (a change moved the measure;
keep watching), **fixed** (the measure has stayed clean across recent runs of each model checked).

## Register

| Seen | Weakness | Measured by | Latest | Status |
| --- | --- | --- | --- | --- |
| 2026-11-10 live check, `beam-free` | Declares a goal complete after one turn with most work missing and the check failing. Goal mode trusts the model's own claim. | `evals/soak` · `false_reached` | 1/1 (beam-free, 78 of 120 items missing) | open |
| 2026-11-10 live check, `iceman-contributor-free` | Tells the person to look "under the `personal` plugin", which Integrations shows as Yours. | `evals/integrations` · `holder_named` | guides (#369): main 11/21 → 6/21 on iceman; the rest quotes `doctor`'s `personal/notes` | improving |
| 2026-11-10 live check, `iceman-contributor-free` | After the person denies a command, asks two questions at once about retrying it, and asks again after being told no. | not measured yet: needs a scenario where an approval is denied (`evals/integrations` allows every approval) | – | needs an eval |
| 2026-11-10 soak, `beam-free` | Calls `read_file` on a folder. | `evals/soak` · `tool_errors_model` (`not_a_file`) | 1 in the run | open |
| 2026-11-09 integrations eval | Saves a team skill into its own store instead of the project's `.agents/skills`. | `evals/integrations` · `project-skill` scenario | main 2/3 → #355 3/3 | improving |
| 2026-11-09 live check | A hook guard looking for a word misses it in a shell call's `args` list. | `evals/integrations` · `add-hook` scenario | guide fix in #355; see its A/B comment | watching |
| 2026-11-09 live check | Leaves temporary files in the project (for example `scratchpad/glob-greet.json`). | `evals/integrations` · `stray_files` | 3 in 21 runs on main, 2 on #369 (iceman, 2026-11-10) | open |
| ongoing | Says the work is done when the check is not met. | `evals/integrations` · `false_claim`; `evals/verify` · false claims | 0 in the 2026-11-09 A/B | watching |
| ongoing | Redoes or rereads work after a checkpoint. | `evals/soak` · `redone_items`, `rereads_after_checkpoint` | soak trend | watching |
| ongoing | Finishes without checking its last change, or without reproducing a bug first. | `evals/verify` · verification habits | verify trend | watching |
