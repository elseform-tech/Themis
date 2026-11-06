# Code-health ratchets

`python3 -m quality_dashboard.ratchets` holds the product code (`crates/` and `web/src/`, Rust and TypeScript) to size, complexity and repeated-code limits. CI runs it on every pull request and on `main`, in the `dashboard-quality` job, and a failure blocks the merge.

A ratchet only turns one way. New code must meet the limits. Code that already broke a limit when the ratchet started is listed in `quality_dashboard/ratchets_baseline.json` at its size then: it may shrink, never grow, and every gain is locked in, so the list only gets shorter.

## Limits

| What | Limit | Counted |
| --- | --- | --- |
| Lines of a source file | 1000 | Every line outside tests. Inline `#[cfg(test)]` items (including `cfg(all(test, …))`) are left out. |
| Lines of any file, tests included | 2000 | Every line. Applies to test files (`tests/`, `*.test.ts(x)`, `*_e2e.*`, files of `#[cfg(test)] mod name;`) and to a source file together with its inline tests. |
| Lines of a function | 100 | Its own lines. A named function declared inside it is measured on its own; closures and arrow callbacks belong to the function around them. |
| Cognitive complexity of a function | 25 | Branches and loops, weighted by how deeply they nest, so it also catches deep nesting. |
| Cyclomatic complexity of a function | 20 | Independent paths through the function. |
| Repeated code | the recorded total | Lines of production code covered by a run of 60 or more tokens over at least 5 lines that appears more than once. Tests are not counted. The total may not grow. |

Repeated code means exact copies of the token sequence: near-copies that differ only in names or text strings are not counted. Checked against jscpd 4.0.5 (`--min-tokens 60 --min-lines 5`, with its default skip of files over 1000 lines turned off), 92% of the lines this check counts are also found by jscpd; jscpd finds more because its tokens are smaller and it also matches some near-copies.

Functions in test files and inline test modules are not held to the function limits. Complexity comes from `rust-code-analysis-cli` 0.0.25 (`cargo install rust-code-analysis-cli --version 0.0.25 --locked`), the same analyzer the quality dashboard uses; the check stops with an error rather than guessing when it is missing.

## Code-health score

The check ends with a 0–100 score and its three parts, and the quality dashboard shows it on the overview with its trend (one point per commit, kept in the ignored `.quality-dashboard/`):

- **files**: the share of product code lines (tests left out) in files within the size limits;
- **functions**: the share of function lines in functions within every function limit;
- **repeated**: 100 with no repeated code, falling 20 points per percent of repeated lines, so 5% or more scores 0.

The score is their mean. It shows progress; it does not gate anything (the ratchets do), and it measures structure only: whether the code works is for the tests and the live check.

## When it fails

- `GREW … > limit`: new code broke a limit. Split the file or function, or reuse the code that already exists. Never add an entry to the baseline by hand to get past it.
- `GREW … > recorded`: code on the list got bigger. Shrink it back, or move the new part into its own file or function.
- `GREW repeated code`: it names the files with more repeated lines than before. Reuse the existing code instead of copying it.
- `IMPROVED …`: code got better. Run `python3 -m quality_dashboard.ratchets --update` and commit the baseline, so the gain cannot be lost.

`--update` only lowers or removes recorded values. A function or file that moves (for example, while splitting a big file) keeps its allowance when a listed entry disappears in the same change: a function must keep its name, and nothing it carries may be bigger than before. New files and functions get no allowance.

Two pull requests that each pass on their own can together fail on `main` (one shrinks a file and locks it in, the other added lines to it). Fix `main` by shrinking the file again, not by raising the recorded number.

## Splitting big files

Splitting a listed file or function is a refactor: behaviour must not change. Do one file per pull request, keep the moved code as it was, and before calling it done run the full checks from `AGENTS.md`, the CLI end-to-end tests for the area, and the real-app check of the touched features from [live-verification.md](live-verification.md).

## Bugs become rules

When a fix lands, ask whether a static check could have caught the bug before it ran. If the bug belongs to a kind that can come back elsewhere, turn on the Clippy or ESLint rule that catches it, or add a small check, and list it below. A one-off mistake only needs its regression test.

| Bug | Rule | Enforced by |
| --- | --- | --- |
