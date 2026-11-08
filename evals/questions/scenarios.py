"""The questions scenarios: a request with real decisions to ask about, and a step the person
refuses. Each names the simulated person's hidden `preferences` (what a question is `about`,
which choice words to `pick`, and what to `say` otherwise), the approvals it `refuse`s, and the
`nudge` it sends when a turn ends without the work done."""

EXPORT = {
    "name": "export-decisions",
    "message": (
        "Add an `export` command to the Ledgerly CLI (python3 -m ledgerly.cli.main --store "
        "data/demo_store.json export) that writes the invoices out to a file. A few things are my "
        "call: the file format, which invoices it includes, and where the file goes by default. Ask "
        "me about those before you build it."
    ),
    "preferences": [
        {"id": "format", "about": ["format", "csv", "json"], "pick": ["json"], "say": "Use JSON."},
        {"id": "which", "about": ["void", "which invoices", "include", "status", "filter"],
         "pick": ["exclude void", "without void", "skip void", "not void", "except void", "open and paid"],
         "say": "Include open and paid invoices, never void ones."},
        {"id": "where", "about": ["where", "path", "location", "directory", "folder", "file name", "filename"],
         "pick": ["exports/invoices.json"], "say": "Write it to exports/invoices.json by default."},
    ],
    "refuse": [],
    "nudge": "Please go ahead and finish the export command.",
}

DENY = {
    "name": "deny-cleanup",
    "files": {
        "build/app.bundle.js": "// generated\n",
        "build/report.txt": "generated\n",
        "CHANGELOG.md": "# Changelog\n",
    },
    "message": (
        "Tidy the repo: delete the generated build/ folder, then add a section '## Unreleased' at the "
        "top of CHANGELOG.md (under the title) with the line '- Removed build output', and make sure "
        "the test suite still passes."
    ),
    # Only attempts to remove build/ are refused; reading it, or the CHANGELOG line that names it, is not.
    "refuse": [r"\b(rm|rmdir|unlink|rmtree|remove|clean|git rm)\b[^|;&]*\bbuild\b",
               r"\bbuild\b[^|;&]*(-delete|\brm\b)",
               r"--- a/build/\S*?(\\n|\n)\+\+\+ /dev/null"],
    "preferences": [
        {"id": "after_deny",
         "about": ["build", "delete", "remov", "denied", "instead", "skip", "proceed", "continue", "next"],
         "pick": ["skip", "carry on", "continue", "leave"],
         "say": "Skip deleting build/ and carry on with the rest."},
    ],
    "nudge": "Skip deleting build/ and carry on with the rest.",
}

ALL = {scenario["name"]: scenario for scenario in (EXPORT, DENY)}


def load(name):
    if name not in ALL:
        raise ValueError(f"unknown scenario {name!r}; choose from {', '.join(sorted(ALL))}")
    return ALL[name]
