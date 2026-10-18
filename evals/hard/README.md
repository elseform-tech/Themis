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

## Tasks

| Job | What the agent builds | How it is graded |
| --- | --- | --- |
| `threejs` | A three.js night city: `src/scene.js` exporting `createScene(THREE)` (40+ tagged buildings, 8+ heights, glowing windows, two lights, fog, an orbiting camera) and a full-window `index.html` | The grader's own page imports the agent's module in headless Chrome (WebGL on SwiftShader), counts buildings, heights, emissive materials, lights and fog, checks the camera moves, renders a frame and checks it reads as night with glowing windows; `index.html` must show a canvas without console errors |
| `sheet` | A spreadsheet engine in Python: formulas with precedence, ranges, SUM/AVG/MIN/MAX/COUNT/IF/ABS/ROUND, error values (`#DIV/0!`, `#REF!`, `#CYCLE!`, `#NAME?`, `#VALUE!`, `#ERROR!`), recalculation, CSV and a command line, with its own tests | 36 hidden test methods in a sandboxed subprocess (no network, no access under `/Users`), plus the agent's own tests (at least 10, passing) |

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
