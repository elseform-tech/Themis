# Live verification after a merge

Automated tests use mock providers. After every merge to `main`, a live check confirms that the
freshly built app works with real models: a sanity check that it starts and answers, and a long
goal that shows whether goal mode, compaction and the tools still hold up. It is a **benchmark audit**, reported separately from
the automated test inventory.

Never use real app data or the real Keychain: every step below runs with its own HOME and data
folder, and the key comes from `OPENCODE_KEY` (the app falls back to it when no Keychain entry is
found). Use free or cheap models (for example `muse-spark-1.3-contributor`, `mimo-v2.6-flash`,
`qwen3.8-flash`, `glm-5.3-flash`, `deepseek-v4.1-flash`).

## 1. Sync and build from `main`

```sh
git checkout main && git pull --ff-only
npm --prefix web run build
cargo build -p themis-desktop --features tauri/custom-protocol   # desktop app with the built UI
cargo build -p themis-desktop --bin themis                       # CLI and app server
```

## 2. The desktop app, by hand

Launch the fresh build with a scratch HOME (for desktop control, wrap the binary in a scratch
`.app` with a test bundle identifier first):

```sh
HOME=$(mktemp -d) target/debug/themis-desktop
```

In a new chat on a cheap thinking model, send a task that writes and reads a file (approve the
write), then a follow-up that depends on the first answer. Check: both answers are right, the
approval prompt appears once per write, the activity list shows only public narration and tool
calls, and the follow-up keeps the context. Quit the app.

## 3. The goal soak

```sh
python3 -m evals.soak run --model muse-spark-1.3-contributor
```

`evals/soak` runs one long goal through the real app server and CLI: 120 prose memos (about 196k
tokens) read and written one at a time under a 64k budget, so the run passes many checkpoints, a
follow-up is queued midway, and three recall questions follow the goal. It tests our machinery,
not the model's skill: goal turns continue, checkpoints succeed and keep the first message's rules,
tools behave. Runs take 20–90 minutes and about $0.30–0.60, under a $1.50 per-run cap. The result
is a trend against the last 5 runs of the same model, budget and job: a **breakage** flag on a
harness measure is our bug; a **regression** flag on a model measure asks a person to look at the
trace. Commit the run's `history.jsonl` line with the report. See `evals/soak/README.md`.

## Reporting and fixing

Report each step with its exact command and result (the soak's flags, its end state, lost runs
and their reasons). Scan the scratch folders for saved reasoning or the key. A bug found here is fixed on
its own `codex/<task>` branch with a regression test (and a CLI end-to-end case for backend
changes), then merged and verified the same way.
