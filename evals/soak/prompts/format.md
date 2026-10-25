# Output format

Each source file `src/<name>.txt` is a memo about one item. Read it and write `out/<name>.md` with these lines:

```
# <name>
- count: <the number of records it holds now, in digits>
- status: <active, paused or retired: what it is now>
- summary: <one sentence on what it is for>
```

Memos describe things in their own words: work out each value from what the memo says, not from the first number or team you see.

`out/INDEX.md` lists every item, one line each, sorted by name, then the total of every item's count (retired items included):

```
- <name>: <status>
total: <sum of all counts>
```

`python3 check.py` checks the structure of these files and the total; it does not check the other values.

In this project, read each memo with `read_file` and write each file with `write_file` (or edit it with `apply_patch`). The only command that runs is `python3 check.py`; other commands, scripts and searches are blocked.