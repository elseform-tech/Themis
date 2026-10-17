# Live verification after a merge

Automated tests use mock providers. After every merge to `main`, a live check confirms that the
freshly built app works with real models: a sanity check that it starts and answers, and a quality
check that agent behaviour has not regressed. It is a **benchmark audit**, reported separately from
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

## 3. Long tasks through the real app

```sh
python3 -m evals.hard run --via app --model muse-spark-1.3-contributor --memory both
```

`evals/hard` runs a three.js scene and a spreadsheet engine as real app chats (`themis` app server
and CLI, YOLO approvals, the app's OS sandbox), with the app's memory and with a 32,000-token
budget that forces checkpoints. See `evals/hard/README.md`.

## 4. Short tasks

```sh
python3 -m evals.tools run --model muse-spark-1.3-contributor
```

## Reporting and fixing

Report each step with its exact command and result (pass rates, failures, lost runs and their
reasons). Scan the scratch folders for saved reasoning or the key. A bug found here is fixed on
its own `codex/<task>` branch with a regression test (and a CLI end-to-end case for backend
changes), then merged and verified the same way.
