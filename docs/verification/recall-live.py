#!/usr/bin/env python3
"""Live recall comparison (NOT part of CI): one scenario against a real free model.

Usage: recall-live.py BINARY LABEL OUTPUT.json [--scratch DIR]

Everything runs in an isolated HOME and --data-dir under a scratch directory.
OPENCODE_KEY is inherited from the environment and never printed. The script
pastes a deterministic multi-chapter story (>= 40 KB) into a chat, adds filler to
force compaction at context_token_budget 24000, then asks five detail questions
that only the original text answers.
"""
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

MODEL = "longcat-2.5-preview-free"

FACTS = [
    # (chapter, sentence, question, accepted answers)
    (3, "On the keeper's cottage the number 4172 was painted in white on the green door.",
     "What number was painted on the door of the keeper's cottage in the story?", ["4172"]),
    (5, "The ship that ran aground that night was the Marigold Tern, out of Skarholm.",
     "What was the name of the ship that ran aground in the story?", ["marigold tern"]),
    (7, "Ingrid Halvorsen left behind a saffron scarf on the lighthouse stairs.",
     "What colour was the scarf that Ingrid Halvorsen left behind?", ["saffron"]),
    (9, "The harbour toll that season was set at 83 crowns per vessel.",
     "How many crowns was the harbour toll per vessel?", ["83"]),
    (11, "Orsolya Vance hid the brass key under the third step of the bell tower.",
     "Who hid the brass key, and where was it hidden?", ["orsolya vance"]),
]

NOUNS = "gull net rope lantern barrel tide cliff gale mast anchor wharf fog cart shingle kettle ledger".split()
ADJ = "grey salt-worn quiet restless narrow weathered distant pale heavy slow".split()
VERBS = "drifted waited counted watched mended carried whispered lingered turned gathered".split()
NAMES = "Ansel Brigid Corvin Dalia Edmar Fenna Garrick Hesper Ivo Jolene".split()


def story(seed=7):
    rng = random.Random(seed)
    chapters = []
    for number in range(1, 13):
        sentences = []
        for _ in range(50):
            sentences.append(
                f"The {rng.choice(ADJ)} {rng.choice(NOUNS)} {rng.choice(VERBS)} while "
                f"{rng.choice(NAMES)} {rng.choice(VERBS)} beside the {rng.choice(ADJ)} {rng.choice(NOUNS)}."
            )
        for chapter, sentence, _, _ in FACTS:
            if chapter == number:
                sentences.insert(rng.randrange(8, 26), sentence)
        chapters.append(f"Chapter {number}\n" + " ".join(sentences))
    return "\n\n".join(chapters)


def filler(seed=11):
    rng = random.Random(seed)
    return "\n".join(
        f"log {i}: {rng.choice(NOUNS)} {rng.randrange(1000, 9999)} {rng.choice(ADJ)} {rng.choice(VERBS)}"
        for i in range(500)
    )


class Chat:
    def __init__(self, binary, root):
        self.binary = binary
        self.root = Path(root)
        self.home = self.root / "home"
        self.data = self.root / "data"
        self.proj = self.root / "proj"
        for p in (self.home, self.data, self.proj):
            p.mkdir(parents=True, exist_ok=True)
        (self.data / "runtime.jsonc").write_text(
            '{"context_token_budget":24000,"recent_messages":4,"total_turns":24}\n'
        )
        self.env = dict(os.environ, HOME=str(self.home))

    def run(self, *args, timeout=900):
        return subprocess.run(
            [self.binary, "--data-dir", str(self.data), *args],
            capture_output=True, text=True, env=self.env, timeout=timeout,
        )

    def create(self):
        out = self.run("thread", "create", str(self.proj), MODEL)
        self.id = json.loads(out.stdout)["id"]

    def send(self, text):
        started = time.time()
        out = self.run("thread", "send", self.id, text, "--json")
        events = [json.loads(l)["payload"] for l in out.stdout.splitlines() if l.startswith("{")]
        tools, checkpoints, result, kind, system_tokens, approvals = [], 0, "", "", None, 0
        for payload in events:
            event = payload.get("event", {})
            k = event.get("kind")
            if k == "tool_started":
                tools.append(event.get("tool") or event.get("name") or "?")
            elif k == "context_checkpoint":
                checkpoints += 1
            elif k == "context_budget" and system_tokens is None:
                system_tokens = event["budget"]["breakdown"]["system"]
            elif k in ("finished", "failed", "incomplete"):
                kind = k
                result = event.get("result") or event.get("error") or ""
        return {"kind": kind, "result": result, "tools": tools, "checkpoints": checkpoints,
                "system_tokens_first_request": system_tokens, "seconds": round(time.time() - started, 1),
                "stderr": out.stderr[-300:]}

    def stop_server(self):
        self.run("server", "stop", timeout=60)

    def evidence_dir(self):
        return self.proj / ".themis" / "context" / self.id


def check_citation(answer, evidence, expected):
    """Valid when the answer names an existing saved file+record whose field contains the fact."""
    names = re.findall(r"(snapshot-\d+\.jsonl|history-source-[\w-]+\.jsonl)", answer)
    records = [int(n) for n in re.findall(r"(?:record|line|jsonl_record)\D{0,6}(\d+)", answer, re.I)]
    if not names or not records:
        return "none"
    for name in names:
        path = evidence / name
        if not path.is_file():
            continue
        lines = path.read_text(errors="replace").splitlines()
        for number in records:
            if 1 <= number <= len(lines) and any(e in lines[number - 1].lower() for e in expected):
                return "valid"
    return "invalid"


def main():
    binary, label, output = sys.argv[1:4]
    scratch = Path(sys.argv[5]) if len(sys.argv) > 5 and sys.argv[4] == "--scratch" else Path(tempfile.mkdtemp(prefix="recall-live-"))
    chat = Chat(binary, scratch / label)
    chat.create()
    text = story()
    report = {"label": label, "binary": binary, "model": MODEL, "story_bytes": len(text.encode()), "steps": [], "questions": []}
    try:
        report["steps"].append(("story", chat.send(
            "Here is a story I am writing. Read it, then reply only 'received'.\n\n" + text)))
        report["steps"].append(("filler", chat.send(
            "Unrelated shipping log, ignore it and reply only 'logged'.\n\n" + filler())))
        for _, _, question, expected in FACTS:
            ask = (question + " Answer from the original story text, quote the sentence, and cite the saved "
                   "source file name and record number you used.")
            result = chat.send(ask)
            answer = result["result"]
            result.update({
                "question": question,
                "correct": any(e in answer.lower() for e in expected),
                "recall_used": "recall" in result["tools"],
                "tool_calls": len(result["tools"]),
                "citation": check_citation(answer, chat.evidence_dir(), expected),
            })
            report["questions"].append(result)
        evidence = chat.evidence_dir()
        report["evidence_files"] = sorted(p.name for p in evidence.glob("*.jsonl"))
    finally:
        chat.stop_server()
    Path(output).write_text(json.dumps(report, indent=2))
    for q in report["questions"]:
        print(f"{label}: correct={q['correct']} recall={q['recall_used']} calls={q['tool_calls']} "
              f"citation={q['citation']} | {q['question'][:50]}")
    shutil.rmtree(scratch / label / "home", ignore_errors=True)


if __name__ == "__main__":
    main()
