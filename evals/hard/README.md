# Hard tasks

Two long, difficult single-message jobs, run by default through the **real app**: the `themis`
app server and CLI built from this checkout, a real chat with YOLO approvals, and the app's own OS
sandbox for the agent's commands. **Live-model benchmark audit; not an automated runtime check.**
The offline grader self-tests (`python3 -m unittest evals.hard.test_harness`) are the automated part.

```sh
python3 -m evals.hard run --model mimo-v2.6-flash --memory both     # via the real app
python3 -m evals.hard run --via runner --model mimo-v2.6-flash      # via the tool-eval runner
python3 -m evals.hard run --themis path/to/themis --job threejs      # a prebuilt CLI, e.g. a fix branch's
python3 -m evals.hard report evals/hard/results/<file>.json
```

## Running and comparing

```sh
# The default jobs (sheet, threejs) through the real app server and CLI:
python3 -m evals.hard run --model glm-5.3-flash --trials 3 --out /tmp/glm.json
# One named job; long tasks want ordinary memory:
python3 -m evals.hard run --job sheet-long --memory normal --model glm-5.3-flash --out /tmp/long.json
python3 -m evals.hard run --job codebase --memory normal --model glm-5.3-flash --out /tmp/code.json
# Runtime A/B with the tool-eval runner (eval switches, notes off as a second arm):
python3 -m evals.hard run --via runner --nudges-off=working_files --no-notes --out /tmp/off.json
# Side by side, with pass rate, stages, calls per pass and the checkpoint measures:
python3 -m evals.hard compare before=/tmp/off.json after=/tmp/glm.json
```

`OPENCODE_KEY` must be set in the environment. Each run uses a fresh scratch project and HOME;
`--keep DIR` saves the runs' `.themis/` evidence. `report FILE` prints one file's full table.

## Tasks

| Job | What the agent builds | How it is graded |
| --- | --- | --- |
| `threejs` | A three.js night city: `src/scene.js` exporting `createScene(THREE)` (40+ tagged buildings, 8+ heights, glowing windows, two lights, fog, an orbiting camera) and a full-window `index.html` | The grader's own page imports the agent's module in headless Chrome (WebGL on SwiftShader), counts buildings, heights, emissive materials, lights and fog, checks the camera moves, renders a frame and checks it reads as night with glowing windows; `index.html` must show a canvas without console errors |
| `sheet` | A spreadsheet engine in Python: formulas with precedence, ranges, SUM/AVG/MIN/MAX/COUNT/IF/ABS/ROUND, error values (`#DIV/0!`, `#REF!`, `#CYCLE!`, `#NAME?`, `#VALUE!`, `#ERROR!`), recalculation, CSV and a command line, with its own tests | 36 hidden test methods in a sandboxed subprocess (no network, no access under `/Users`), plus the agent's own tests (at least 10, passing) |
| `sheet-long` (run when named: `--job sheet-long`, best with `--memory normal`) | The `sheet` task, then three follow-up messages in the same chat that build on it: named references, text in formulas (string literals, CONCAT, LEN, UPPER, LOWER, IFERROR), and a command line with `eval --raw` and `set` | One hidden suite at the end with the 36 `sheet` tests and 9 for the follow-ups, so forgetting an earlier requirement fails too; reported per stage (engine, names, text, command line) as `extra.stages_passed`, plus the agent's own tests |
| `codebase` (run when named) | Two reported bugs fixed and a `getlist` feature added in `iniconf`, a 1,400-line INI parser (Python's configparser, see `vendor/iniconf/NOTICE.md`) the agent did not write, keeping its 344-test suite passing, with tests added | The original suite (pristine, so editing the project's copy does not help) and 6 hidden tests, reported per stage (suite, percent bug, vars bug, getlist), plus the agent's own suite, which must pass and have grown |

The three.js copy in `vendor/three/` (MIT, r182) is the only three.js the agent has; the grader
uses its own copy, so the agent cannot change what is measured. `reference/` holds the grader's
reference solutions, which the self-tests check pass.

## Setups

- Memory: `normal` uses the app's defaults (200,000 tokens); `squeezed` sets
  `context_token_budget` 32,000 in the run's `runtime.jsonc`, forcing checkpoints mid-task, so
  compaction is exercised.
- Each run has its own data folder, HOME and scratch git project; the key comes from
  `OPENCODE_KEY`. A run lost to the provider is rerun once after two minutes (`--no-retry` turns
  that off). Results are saved after every run; `--keep DIR` keeps each run's data folder and
  project.

## Limits

One message per job; the specs are exact, so this measures building to a spec over a long run,
not design judgement. Visual checks are coarse (counts and pixel statistics), not a judgement of
how good the scene looks.
