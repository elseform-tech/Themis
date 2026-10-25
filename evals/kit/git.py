"""Git helpers for scratch projects (copied from `evals/tools/tasks.py`)."""

import subprocess


def git(project, *args):
    return subprocess.run(["git", *args], cwd=project, capture_output=True, text=True, check=False).stdout


def commit_all(project, message):
    git(project, "add", "-A")
    git(project, "-c", "user.email=eval@example.com", "-c", "user.name=Eval", "commit", "-q", "-m", message)


def init(project, message="Scratch project"):
    """A fresh repository holding everything in `project` as one commit."""
    git(project, "init", "-q")
    commit_all(project, message)


def commits(project):
    """Commit count on the current branch (0 when there is none)."""
    out = git(project, "rev-list", "--count", "HEAD").strip()
    return int(out) if out.isdigit() else 0


def dirty(project):
    """Whether the working tree has uncommitted changes."""
    return bool(git(project, "status", "--porcelain").strip())
