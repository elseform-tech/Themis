# Output format

For each source file `src/<name>.txt`, write `out/<name>.md` with these lines, taking the values from the source file:

```
# <name>
- region: <region>
- count: <count>
- status: <status>
```

`out/INDEX.md` lists every item, one line each, sorted by name, then a total of every item's count:

```
- <name>: <status>
total: <sum of all counts>
```

`python3 check.py` checks the facts in these files.