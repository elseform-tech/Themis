# Tool-use eval

Measures how well the agent uses each tool on the first try, and how many calls it wastes.
**Live-model benchmark audit; not an automated runtime check.** The offline harness tests
(`python3 -m unittest evals.tools.test_harness`) are the automated part.

## What it does

Each task in `tasks.py` builds a small scratch project (always a git repository), asks for one
job and checks the end state: files on disk, the answer, or that a refused action stayed
undone. The runner (`crates/core/examples/tool_eval.rs`, `runtime/tool_eval.rs`) gives the
agent the real chat toolset (file tools, `shell`, background tasks, `apply_patch`, `git`, and
`recall` for the task that has saved originals). Approvals act like a person in Custom mode who
allows every non-destructive action once; destructive ones are refused.

Every run gets a fresh project in the system temp folder, a scratch HOME and, on macOS,
`sandbox-exec` around the runner (reads and writes under `/Users` and `/Volumes` blocked;
network open), as in the long-form suite. Themis' own sandbox is off inside the runner because
macOS does not allow nested sandboxes.

## Tasks

| Tool | Tasks |
| --- | --- |
| `read_file` | small file; one FATAL line deep in a 3,000-line log; one record of a JSONL file |
| `list_dir` / `search_file` | find a file by name; find the file that defines a function |
| `write_file` | create a file; append a line without touching the rest |
| `apply_patch` | change two separate spots; edit a tab-indented Makefile line |
| `shell` | run a script with arguments; list a subfolder; grep a file whose name has a space |
| `background_task` | start a server, fetch its page, stop it |
| `git` | which file changed; latest commit subject; a refused `git reset --hard` that must stay undone, with no workaround and the user told |
| `recall` | a detail from compacted history |
| `notes` | a three-file job that asks for notes (no open task lines at the end); a four-turn chat that does part of a job, answers an unrelated question, reports what is left without doing it, then finishes |

## Metrics

Every call is classified as a success or a wasted call with a reason: `bad_arguments`,
`wrong_path`, `patch_format`, `command_failed` (shell exit code not 0), `empty_result` (recall
found nothing), `refused`, `other_error`. Reported per tool: calls, **first-try success** (the
first call of that tool in a task), wasted share and reasons; overall: task pass rate, first-try
success, wasted-call share, calls, input tokens and latency per task; per task: pass rate and
the first failure. Results keep a 300-character output excerpt for wasted calls only.

## Run

```sh
export OPENCODE_KEY=...   # never commit or print it
python3 -m evals.tools run --trials 3 --model longcat-2.5-preview-free
python3 -m evals.tools run --trials 3 --build-ref main      # the same tasks against main's tools
python3 -m evals.tools report evals/tools/results/<file>.json
```

Options: `--task NAME` (repeatable), `--jobs N` (default 2), `--max-turns N`, `--timeout S`,
`--runner PATH`, `--sandbox auto|off`, `--out FILE`, `--base-url URL` (mock provider).
`--build-ref` overlays only `runtime/tool_eval.rs`, the example and its Cargo stanza onto the
ref, so the tools under test are the ref's own.

Limits: one free model, small synthetic projects, lexical checks. Automations, integrations
and skills (`manage_*`, `save_skill`) live in the desktop app server and are not covered yet.
