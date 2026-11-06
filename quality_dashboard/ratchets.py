"""Code-health ratchets: limits new code must meet and old code may only move toward.

Run from the repository root:

    python3 -m quality_dashboard.ratchets            # check (CI runs this)
    python3 -m quality_dashboard.ratchets --update   # lock in improvements

Every file and function is held to the limits in ``LIMITS``. Code that already
broke a limit when the ratchet started is listed in ``ratchets_baseline.json``
at its size then; it may shrink but never grow. ``--update`` only lowers or
removes recorded values, so the baseline can only get stricter.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

from quality_dashboard.dashboard import (
    AST_ANALYZER_VERSION,
    ROOT,
    SOURCE_ROOTS,
    _parse_ast_output,
    is_test_file,
    read_text,
)

BASELINE_PATH = Path(__file__).resolve().parent / "ratchets_baseline.json"
CODE_SUFFIXES = {".rs", ".ts", ".tsx", ".js", ".jsx"}

LIMITS = {
    # Lines of a source file outside tests (inline `#[cfg(test)]` items are not counted).
    "file_code_lines": 1000,
    # All lines of any file, tests included: a test file, or a source file with its tests.
    "file_total_lines": 2000,
    # Per named function or method, not counting named functions declared inside it.
    "function_lines": 100,
    "function_cognitive": 25,
    "function_cyclomatic": 20,
}
# Repeated code: a run of at least this many tokens that appears twice or more in
# production code (tests excluded). The whole product's repeated lines may not grow.
DUPLICATE_MIN_TOKENS = 60
DUPLICATE_MIN_LINES = 5

UPDATE_HINT = "python3 -m quality_dashboard.ratchets --update"


# --- Tokenizing ---------------------------------------------------------------------------


@dataclass
class Token:
    kind: str  # "ident", "num", "str", "punct"
    text: str
    line: int


_PUNCT_BEFORE_VALUE = {")", "]", "}"}
_KEYWORDS_BEFORE_REGEX = {"return", "typeof", "case", "do", "else", "in", "of", "void", "yield", "await"}


class _Lexer:
    """A small C-family lexer for Rust and TypeScript.

    It only needs to find identifiers, punctuation and the extent of strings and
    comments, so that braces inside them are not mistaken for code structure.
    """

    def __init__(self, text: str, rust: bool) -> None:
        self.text = text
        self.rust = rust
        self.pos = 0
        self.line = 1
        self.tokens: list[Token] = []
        self.braces: list[str] = []  # TypeScript: "code" or "template" per open `{` / `${`

    def run(self) -> list[Token]:
        text = self.text
        while self.pos < len(text):
            char = text[self.pos]
            if char == "\n":
                self.line += 1
                self.pos += 1
            elif char.isspace():
                self.pos += 1
            elif text.startswith("//", self.pos):
                end = text.find("\n", self.pos)
                self.pos = len(text) if end < 0 else end
            elif text.startswith("/*", self.pos):
                self._block_comment()
            elif char == "'" and not self.rust and self._after_word():
                # An apostrophe in JSX text.
                self._emit("punct", char, self.pos + 1)
            elif char == '"' or (char == "'" and not self.rust):
                self._quoted(char)
            elif char == "`" and not self.rust:
                self._template(self.pos + 1)
            elif char == "{" and not self.rust:
                self._open_brace()
            elif char == "}" and not self.rust:
                self._close_brace()
            elif char == "'":
                self._rust_quote()
            elif char == "/" and self._regex_allowed():
                self._regex()
            elif char.isalpha() or char == "_" or char == "$":
                self._word()
            elif char.isdigit():
                self._number()
            else:
                self._emit("punct", char, self.pos + 1)
        return self.tokens

    def _after_word(self) -> bool:
        # "chat's", or "{name}'s" after a JSX expression: code never opens a string there.
        return self.pos > 0 and (self.text[self.pos - 1].isalnum() or self.text[self.pos - 1] == "}")

    def _emit(self, kind: str, text: str, end: int) -> None:
        self.tokens.append(Token(kind, text, self.line))
        self._advance_to(end)

    def _advance_to(self, end: int) -> None:
        self.line += self.text.count("\n", self.pos, end)
        self.pos = end

    def _block_comment(self) -> None:
        depth = 0
        index = self.pos
        text = self.text
        while index < len(text):
            if text.startswith("/*", index):
                depth += 1
                index += 2
                if not self.rust and depth > 1:
                    depth = 1
            elif text.startswith("*/", index):
                depth -= 1
                index += 2
                if depth == 0:
                    break
            else:
                index += 1
        self._advance_to(index)

    def _string_end(self, start: int, quote: str) -> int:
        index = start
        text = self.text
        while index < len(text):
            char = text[index]
            if char == "\\":
                index += 2
                continue
            if char == quote:
                return index + 1
            if char == "\n" and quote == "'" and not self.rust:
                return index
            index += 1
        return len(text)

    def _quoted(self, quote: str) -> None:
        start = self.pos
        end = self._string_end(start + 1, quote)
        self._emit("str", self.text[start:end], end)

    def _template(self, start: int) -> None:
        """Template text from ``start`` up to its closing backtick or the next `${`.

        Code inside `${...}` is lexed as ordinary code; ``run`` resumes the template
        when the `}` that closes it arrives (tracked on ``self.braces``).
        """
        index = start
        text = self.text
        while index < len(text):
            char = text[index]
            if char == "\\":
                index += 2
                continue
            if char == "`":
                self._emit("str", text[self.pos:index + 1], index + 1)
                return
            if text.startswith("${", index):
                self._emit("str", text[self.pos:index + 2], index + 2)
                self.braces.append("template")
                return
            index += 1
        self._emit("str", text[self.pos:], len(text))

    def _open_brace(self) -> None:
        self.braces.append("code")
        self._emit("punct", "{", self.pos + 1)

    def _close_brace(self) -> None:
        if self.braces and self.braces[-1] == "template":
            self.braces.pop()
            self._template(self.pos + 1)
            return
        if self.braces:
            self.braces.pop()
        self._emit("punct", "}", self.pos + 1)

    def _rust_quote(self) -> None:
        text = self.text
        start = self.pos
        if text.startswith("\\", start + 1):
            end = self._string_end(start + 1, "'")
            self._emit("str", text[start:end], end)
        elif start + 2 < len(text) and text[start + 2] == "'":
            self._emit("str", text[start:start + 3], start + 3)
        else:
            # A lifetime or label such as 'a: the quote, then the name as a word.
            self._emit("punct", "'", start + 1)

    def _raw_string(self, start: int) -> bool:
        """A Rust raw string r#"..."# (with any b/c prefix) starting at ``start``."""
        text = self.text
        index = start
        hashes = 0
        while index < len(text) and text[index] == "#":
            hashes += 1
            index += 1
        if index >= len(text) or text[index] != '"':
            return False
        closing = '"' + "#" * hashes
        end = text.find(closing, index + 1)
        end = len(text) if end < 0 else end + len(closing)
        self._emit("str", text[self.pos:end], end)
        return True

    def _word(self) -> None:
        text = self.text
        index = self.pos
        while index < len(text) and (text[index].isalnum() or text[index] in "_$"):
            index += 1
        word = text[self.pos:index]
        if self.rust and index < len(text):
            if word in {"r", "br", "cr"} and self._raw_string(index):
                return
            if word in {"b", "c"} and text[index] == '"':
                end = self._string_end(index + 1, '"')
                self._emit("str", text[self.pos:end], end)
                return
            if word == "b" and text[index] == "'":
                end = self._string_end(index + 1, "'")
                self._emit("str", text[self.pos:end], end)
                return
        self._emit("ident", word, index)

    def _number(self) -> None:
        text = self.text
        index = self.pos
        while index < len(text) and (text[index].isalnum() or text[index] in "_."):
            if text[index] == "." and not (index + 1 < len(text) and text[index + 1].isdigit()):
                break
            index += 1
        self._emit("num", text[self.pos:index], index)

    def _regex_allowed(self) -> bool:
        if self.rust:
            return False
        if not self.tokens:
            return True
        last = self.tokens[-1]
        if last.kind == "punct":
            # `</div>` is a closing JSX tag, not a regular expression.
            return last.text not in _PUNCT_BEFORE_VALUE and last.text != "<"
        return last.kind == "ident" and last.text in _KEYWORDS_BEFORE_REGEX

    def _regex(self) -> None:
        text = self.text
        index = self.pos + 1
        in_class = False
        while index < len(text) and text[index] != "\n":
            char = text[index]
            if char == "\\":
                index += 2
                continue
            if char == "[":
                in_class = True
            elif char == "]":
                in_class = False
            elif char == "/" and not in_class:
                index += 1
                break
            index += 1
        while index < len(text) and text[index].isalpha():
            index += 1
        self._emit("str", text[self.pos:index], index)


def tokenize(text: str, rust: bool) -> list[Token]:
    return _Lexer(text, rust).run()


# --- Rust test regions --------------------------------------------------------------------


def _attribute_end(tokens: list[Token], start: int) -> int:
    """Index just past the `]` closing the attribute whose `#` is at ``start``."""
    depth = 0
    for index in range(start + 1, len(tokens)):
        if tokens[index].kind != "punct":
            continue
        text = tokens[index].text
        if text == "[":
            depth += 1
        elif text == "]":
            depth -= 1
            if depth == 0:
                return index + 1
    return len(tokens)


def _is_test_cfg(tokens: list[Token], start: int, end: int) -> bool:
    words = [token.text for token in tokens[start:end] if token.kind == "ident"]
    return words[:1] == ["cfg"] and "test" in words and "not" not in words


def _item_end(tokens: list[Token], start: int) -> int:
    """Index of the last token of the item starting at ``start`` (`;` or its closing `}`)."""
    depth = 0
    for index in range(start, len(tokens)):
        if tokens[index].kind != "punct":
            continue
        text = tokens[index].text
        if text in "([{":
            depth += 1
        elif text in ")]}":
            depth -= 1
            if depth == 0 and text == "}":
                return index
        elif text == ";" and depth == 0:
            return index
    return len(tokens) - 1


def _is_attribute_start(tokens: list[Token], index: int) -> bool:
    return (
        tokens[index].text == "#"
        and index + 1 < len(tokens)
        and tokens[index + 1].text in {"[", "!"}
    )


@dataclass
class TestItem:
    first_line: int
    last_line: int
    out_of_line_module: str | None = None


def rust_test_items(tokens: list[Token]) -> list[TestItem]:
    """Items gated by `#[cfg(test)]` (or `cfg(all(test, ...))`), with their line spans."""
    items: list[TestItem] = []
    index = 0
    while index < len(tokens):
        if not (_is_attribute_start(tokens, index) and tokens[index + 1].text == "["):
            index += 1
            continue
        attribute_end = _attribute_end(tokens, index)
        if not _is_test_cfg(tokens, index + 2, attribute_end - 1):
            index = attribute_end
            continue
        item_start = attribute_end
        while item_start < len(tokens) and _is_attribute_start(tokens, item_start):
            item_start = _attribute_end(tokens, item_start)
        if item_start >= len(tokens):
            break
        item_end = _item_end(tokens, item_start)
        module = None
        words = [token.text for token in tokens[item_start:item_end + 1]]
        if words[-1:] == [";"] and "mod" in words:
            module = words[words.index("mod") + 1]
        items.append(TestItem(tokens[index].line, tokens[item_end].line, module))
        index = item_end + 1
    return items


def _rust_module_dir(path: Path) -> Path:
    if path.name in {"lib.rs", "main.rs", "mod.rs"}:
        return path.parent
    return path.parent / path.stem


def out_of_line_test_modules(path: Path, items: list[TestItem]) -> set[Path]:
    """Files holding `#[cfg(test)] mod name;` modules declared in ``path``."""
    modules = set()
    for item in items:
        if item.out_of_line_module is None:
            continue
        base = _rust_module_dir(path) / item.out_of_line_module
        for candidate in (base.with_suffix(".rs"), base / "mod.rs"):
            if candidate.is_file():
                modules.add(candidate.resolve())
    return modules


# --- Measuring ----------------------------------------------------------------------------


def line_total(text: str) -> int:
    return len(text.splitlines())


@dataclass
class SourceFile:
    path: Path
    relative: str
    text: str
    tokens: list[Token]
    test_lines: set[int] = field(default_factory=set)
    is_test: bool = False

    @property
    def rust(self) -> bool:
        return self.path.suffix == ".rs"


def load_sources(paths: list[Path]) -> list[SourceFile]:
    sources = []
    test_modules: set[Path] = set()
    for path in paths:
        text = read_text(path)
        rust = path.suffix == ".rs"
        source = SourceFile(path, path.relative_to(ROOT).as_posix(), text, tokenize(text, rust))
        source.is_test = is_test_file(path)
        if rust:
            items = rust_test_items(source.tokens)
            for item in items:
                source.test_lines.update(range(item.first_line, item.last_line + 1))
            test_modules |= out_of_line_test_modules(path, items)
        sources.append(source)
    for source in sources:
        if source.path.resolve() in test_modules:
            source.is_test = True
    return sources


def code_source_files() -> list[Path]:
    """Product code git knows about: tracked files and new ones not ignored.

    Ignored build output (such as the bundled SearXNG runtime) exists only on some
    machines, so measuring it would make the check differ between a laptop and CI.
    """
    result = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", *SOURCE_ROOTS],
        cwd=ROOT, capture_output=True, text=True, timeout=60, check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"git ls-files failed: {result.stderr.strip()[:200]}")
    paths = {ROOT / name for name in result.stdout.split("\0") if name}
    return sorted(path for path in paths if path.suffix in CODE_SUFFIXES and path.is_file())


def file_measurements(sources: list[SourceFile]) -> dict[str, dict[str, int]]:
    measured = {}
    for source in sources:
        total = line_total(source.text)
        values = {"total_lines": total}
        if not source.is_test:
            values["code_lines"] = total - len(source.test_lines)
        measured[source.relative] = values
    return measured


def _function_rows(space: dict, prefix: list[str], inside_function: bool, rows: list[tuple]) -> None:
    """Walk the analyzer's nested spaces, recording each named function with its path."""
    counts: dict[str, int] = {}
    for child in space.get("spaces", []):
        if not isinstance(child, dict):
            continue
        name = str(child.get("name") or "<anonymous>")
        is_function = child.get("kind") in {"function", "method"}
        if is_function and name == "<anonymous>" and inside_function:
            # Closures and arrow callbacks belong to the function around them.
            _function_rows(child, prefix, True, rows)
            continue
        counts[name] = counts.get(name, 0) + 1
        path = [*prefix, name if counts[name] == 1 else f"{name}#{counts[name]}"]
        if is_function:
            rows.append((path, child))
        _function_rows(child, path, inside_function or is_function, rows)


def _named_children(space: dict) -> list[dict]:
    """Named functions declared inside ``space`` (closures fold into their owner)."""
    found = []
    stack = [child for child in space.get("spaces", []) if isinstance(child, dict)]
    while stack:
        child = stack.pop()
        if child.get("kind") in {"function", "method"} and child.get("name") not in {None, "<anonymous>"}:
            found.append(child)
            continue
        stack.extend(grand for grand in child.get("spaces", []) if isinstance(grand, dict))
    return found


def _metric(space: dict, name: str) -> float:
    metrics = space.get("metrics", {})
    if name == "lines":
        value = metrics.get("loc", {}).get("sloc", 0)
    else:
        value = metrics.get(name, {}).get("sum", 0)
    return float(value) if isinstance(value, (int, float)) else 0.0


def function_values(space: dict) -> dict[str, int]:
    """Own size and complexity of a function: named inner functions are measured apart."""
    inner = _named_children(space)
    values = {}
    for key, metric in (("lines", "lines"), ("cognitive", "cognitive"), ("cyclomatic", "cyclomatic")):
        own = _metric(space, metric) - sum(_metric(child, metric) for child in inner)
        if metric == "cyclomatic":
            # Each inner function's sum includes its own entry point.
            own = max(own, 1.0)
        values[key] = int(round(own))
    return values


def function_measurements(sources: list[SourceFile], executable: str) -> dict[str, dict[str, int]]:
    targets = [source for source in sources if not source.is_test]
    by_path = {source.relative: source for source in targets}
    command = [executable, "-m", "-O", "json", "-j", "2"]
    for relative in sorted(by_path):
        command.extend(("-p", relative))
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=300, check=False)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip()[:400] or f"analyzer exited with {result.returncode}")
    measured = {}
    analyzed = set()
    for record in _parse_ast_output(result.stdout):
        relative = Path(str(record.get("name"))).as_posix()
        source = by_path.get(relative)
        if source is None:
            continue
        analyzed.add(relative)
        rows: list[tuple] = []
        _function_rows(record, [], False, rows)
        for path, space in rows:
            if space.get("start_line") in source.test_lines:
                continue
            measured[f"{relative}::{'::'.join(path)}"] = function_values(space)
    missing = set(by_path) - analyzed
    if missing:
        raise RuntimeError(f"analyzer returned nothing for {len(missing)} files, e.g. {sorted(missing)[0]}")
    return measured


def duplicate_lines(sources: list[SourceFile]) -> dict[str, int]:
    """Lines of production code covered by a token run that also appears elsewhere.

    A repeat is a run of at least ``DUPLICATE_MIN_TOKENS`` identical tokens spanning at
    least ``DUPLICATE_MIN_LINES`` lines. Windows of that many tokens that begin a line
    are hashed once (a rolling hash); a matching pair is then extended to the full
    identical run, and each run is extended only once, so the work grows linearly with
    the amount of code.
    """
    streams: list[tuple[str, list[Token], list[int]]] = []
    vocabulary: dict[str, int] = {}
    for source in sources:
        if source.is_test:
            continue
        tokens = [token for token in source.tokens if token.line not in source.test_lines]
        if len(tokens) >= DUPLICATE_MIN_TOKENS:
            ids = [vocabulary.setdefault(token.text, len(vocabulary) + 1) for token in tokens]
            streams.append((source.relative, tokens, ids))
    covered: dict[int, set[int]] = {}
    for first, second, end in _matching_windows(streams):
        _mark_repeat(streams, first, second, end - second[1], covered)
    return {streams[stream][0]: len(lines) for stream, lines in covered.items()}


def _matching_windows(streams: list[tuple[str, list[Token], list[int]]]):
    """Pairs of (stream, offset) whose next ``DUPLICATE_MIN_TOKENS`` tokens hash alike."""
    seen: dict[int, int] = {}  # window hash -> stream index << 32 | token offset
    extended: dict[tuple[int, int, int], int] = {}  # (stream, stream, shift) -> end offset
    modulus = (1 << 61) - 1
    base = 1_000_003
    width = DUPLICATE_MIN_TOKENS
    top = pow(base, width - 1, modulus)
    for stream_index, (_, tokens, ids) in enumerate(streams):
        value = 0
        for position, token_id in enumerate(ids):
            if position >= width:
                value = (value - ids[position - width] * top) % modulus
            value = (value * base + token_id) % modulus
            start = position - width + 1
            # Copied code starts on a line of its own, so only windows that begin a line
            # are indexed: one entry per line instead of one per token.
            if start < 0 or (start > 0 and tokens[start - 1].line == tokens[start].line):
                continue
            packed = seen.get(value)
            if packed is None:
                seen[value] = stream_index << 32 | start
                continue
            first = (packed >> 32, packed & 0xFFFFFFFF)
            if first[0] == stream_index and start - first[1] < width:
                continue  # an overlapping repeat of the same run, like a long table
            diagonal = (first[0], stream_index, start - first[1])
            if extended.get(diagonal, -1) > start:
                continue  # inside a run already measured
            end = _identical_run_end(streams, first, (stream_index, start))
            extended[diagonal] = end
            if end - start >= width:
                yield first, (stream_index, start), end


def _identical_run_end(streams, first: tuple[int, int], second: tuple[int, int]) -> int:
    """Offset in ``second``'s stream just past the identical run the two start."""
    first_ids = streams[first[0]][2]
    second_ids = streams[second[0]][2]
    shift = second[1] - first[1]
    index = second[1]
    while (
        index < len(second_ids)
        and index - shift < len(first_ids)
        and (first[0] != second[0] or index - shift < second[1])
        and first_ids[index - shift] == second_ids[index]
    ):
        index += 1
    return index


def _mark_repeat(
    streams, first: tuple[int, int], second: tuple[int, int], length: int, covered: dict[int, set[int]],
) -> None:
    spans = []
    for stream, offset in (first, second):
        tokens = streams[stream][1]
        spans.append((stream, tokens[offset].line, tokens[offset + length - 1].line))
    if any(last - first_line + 1 < DUPLICATE_MIN_LINES for _, first_line, last in spans):
        return
    for stream, first_line, last in spans:
        covered.setdefault(stream, set()).update(range(first_line, last + 1))


@dataclass
class Measurements:
    files: dict[str, dict[str, int]]
    functions: dict[str, dict[str, int]]
    duplicate_lines: dict[str, int]


# Repeated code at this share of the product's lines or more scores 0: below it the
# score falls 20 points per percent, so small amounts of copying still show.
REPEATED_ZERO_SHARE = 0.05


def health_score(measured: Measurements) -> dict:
    """A 0-100 code-health score and its three parts, each 0-100.

    - files: share of product code lines (tests left out) in files within the size limits;
    - functions: share of function lines in functions within every function limit;
    - repeated: 100 with no repeated code, falling to 0 at ``REPEATED_ZERO_SHARE``.
    The score is their mean. It is a progress measure, not a gate: the ratchets gate.
    """
    code = {path: values["code_lines"] for path, values in measured.files.items() if "code_lines" in values}
    code_total = sum(code.values())
    oversized = sum(lines for path, lines in code.items() if over_limits(measured.files[path], FILE_LIMITS))
    function_total = sum(values["lines"] for values in measured.functions.values())
    complex_lines = sum(
        values["lines"] for values in measured.functions.values() if over_limits(values, FUNCTION_LIMITS)
    )
    repeated_share = sum(measured.duplicate_lines.values()) / code_total if code_total else 0.0
    parts = {
        "files": 100 * (1 - oversized / code_total) if code_total else 100.0,
        "functions": 100 * (1 - complex_lines / function_total) if function_total else 100.0,
        "repeated": max(0.0, 100 * (1 - repeated_share / REPEATED_ZERO_SHARE)),
    }
    rounded = {name: round(value, 1) for name, value in parts.items()}
    return {
        "score": round(sum(parts.values()) / len(parts), 1),
        **rounded,
        "repeated_share": round(repeated_share, 4),
    }


def describe_health(health: dict) -> str:
    return (
        f"code health {health['score']:.1f}/100 (files {health['files']:.1f}, "
        f"functions {health['functions']:.1f}, repeated {health['repeated']:.1f})"
    )


def measure(paths: list[Path] | None = None) -> Measurements:
    sources = load_sources(paths if paths is not None else code_source_files())
    configured = os.environ.get("RUST_CODE_ANALYSIS_CLI", "rust-code-analysis-cli")
    executable = shutil.which(configured)
    if executable is None:
        raise RuntimeError(
            "rust-code-analysis-cli is required: cargo install rust-code-analysis-cli "
            f"--version {AST_ANALYZER_VERSION} --locked"
        )
    return Measurements(
        files=file_measurements(sources),
        functions=function_measurements(sources, executable),
        duplicate_lines=duplicate_lines(sources),
    )


# --- Comparing with the baseline ----------------------------------------------------------


FILE_LIMITS = {"code_lines": "file_code_lines", "total_lines": "file_total_lines"}
FUNCTION_LIMITS = {"lines": "function_lines", "cognitive": "function_cognitive", "cyclomatic": "function_cyclomatic"}


def over_limits(values: dict[str, int], limits: dict[str, str]) -> dict[str, int]:
    return {
        metric: value
        for metric, value in values.items()
        if metric in limits and value > LIMITS[limits[metric]]
    }


@dataclass
class Report:
    grew: list[str] = field(default_factory=list)  # broke a limit or a recorded allowance
    improved: list[str] = field(default_factory=list)  # better than recorded: lock it in
    baseline: dict = field(default_factory=dict)  # the stricter baseline --update writes

    @property
    def ok(self) -> bool:
        return not self.grew and not self.improved


def _short_name(key: str) -> str:
    return key.rsplit("::", 1)[-1].split("#", 1)[0]


def _fits(values: dict[str, int], allowance: dict[str, int]) -> bool:
    return all(values[metric] <= allowance.get(metric, values[metric]) for metric in values)


def _ratchet_group(
    kind: str,
    measured: dict[str, dict[str, int]],
    recorded: dict[str, dict[str, int]],
    limits: dict[str, str],
    report: Report,
    same_name: bool,
) -> dict[str, dict[str, int]]:
    """Compare one group (files or functions) and return its stricter recorded values."""
    offenders = {key: over_limits(values, limits) for key, values in measured.items()}
    offenders = {key: values for key, values in offenders.items() if values}
    vanished = {key: allowance for key, allowance in recorded.items() if key not in measured}
    stricter: dict[str, dict[str, int]] = {}
    consumed: set[str] = set()  # recorded keys still present and over a limit
    moved: set[str] = set()  # recorded keys whose allowance followed a move
    for key in sorted(offenders):
        values = offenders[key]
        allowance = recorded.get(key)
        if allowance is None:
            moved_from = next(
                (
                    old
                    for old, old_allowance in sorted(vanished.items())
                    if (not same_name or _short_name(old) == _short_name(key)) and _fits(values, old_allowance)
                ),
                None,
            )
            if moved_from is None:
                for metric, value in values.items():
                    report.grew.append(f"{kind} {key}: {metric} {value} > limit {LIMITS[limits[metric]]}")
                continue
            # Moved or renamed: the old allowance follows it.
            allowance = vanished.pop(moved_from)
            moved.add(moved_from)
            report.improved.append(f"{kind} {moved_from} moved to {key}")
        for metric, value in values.items():
            allowed = allowance.get(metric, LIMITS[limits[metric]])
            if value > allowed:
                report.grew.append(f"{kind} {key}: {metric} {value} > recorded {allowed}")
            elif value < allowed:
                report.improved.append(f"{kind} {key}: {metric} {allowed} -> {value}")
        for metric in allowance.keys() - values.keys():
            report.improved.append(f"{kind} {key}: {metric} now within the limit")
        kept = {
            metric: min(value, allowance.get(metric, LIMITS[limits[metric]]))
            for metric, value in values.items()
        }
        kept = {metric: value for metric, value in kept.items() if value > LIMITS[limits[metric]]}
        if kept:
            stricter[key] = kept
        consumed.add(key)
    for key in sorted(set(recorded) - consumed - moved):
        report.improved.append(f"{kind} {key}: within limits or gone")
    return stricter


def compare(measured: Measurements, baseline: dict) -> Report:
    report = Report()
    files = _ratchet_group("file", measured.files, baseline.get("files", {}), FILE_LIMITS, report, False)
    functions = _ratchet_group(
        "function", measured.functions, baseline.get("functions", {}), FUNCTION_LIMITS, report, True,
    )
    recorded_total = baseline.get("duplicate_lines_total", 0)
    by_file = baseline.get("duplicate_lines_by_file", {})
    total = sum(measured.duplicate_lines.values())
    if total > recorded_total:
        report.grew.append(
            f"repeated code: {total} duplicated lines > recorded {recorded_total}; "
            f"more than before in {_duplicate_growth(measured.duplicate_lines, by_file)}"
        )
    elif total < recorded_total:
        report.improved.append(f"repeated code: {recorded_total} -> {total} duplicated lines")
    report.baseline = {
        "files": files,
        "functions": functions,
        "duplicate_lines_total": min(total, recorded_total),
        # Where the repeated lines were, to point at what grew. Only the total is enforced,
        # so moving code between files never trips the check.
        "duplicate_lines_by_file": dict(sorted(measured.duplicate_lines.items()))
        if total <= recorded_total else by_file,
    }
    return report


def _duplicate_growth(current: dict[str, int], recorded: dict[str, int]) -> str:
    grown = sorted(
        ((path, lines - recorded.get(path, 0)) for path, lines in current.items() if lines > recorded.get(path, 0)),
        key=lambda item: -item[1],
    )
    return ", ".join(f"{path} (+{lines})" for path, lines in grown[:5]) or "files that moved"


def load_baseline(path: Path = BASELINE_PATH) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}


def write_baseline(baseline: dict, path: Path = BASELINE_PATH) -> None:
    path.write_text(json.dumps(baseline, indent=1, sort_keys=True) + "\n", encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--update", action="store_true", help="lock in improvements (never loosens)")
    args = parser.parse_args(argv)
    try:
        measured = measure()
    except (RuntimeError, OSError, subprocess.TimeoutExpired, ValueError) as error:
        print(f"ratchets: could not measure: {error}", file=sys.stderr)
        return 2
    report = compare(measured, load_baseline())
    health = describe_health(health_score(measured))
    for line in report.grew:
        print(f"GREW      {line}")
    for line in report.improved:
        print(f"IMPROVED  {line}")
    if args.update:
        write_baseline(report.baseline)
        print(f"ratchets: baseline tightened ({len(report.improved)} improvements locked in)")
        print(health)
        return 1 if report.grew else 0
    if report.grew:
        print(
            "ratchets: new code broke a limit or old code got worse. Split the file or "
            "function, or reuse the repeated code; limits: " + json.dumps(LIMITS),
        )
        print(health)
        return 1
    if report.improved:
        print(f"ratchets: code got better; lock it in with: {UPDATE_HINT}")
        print(health)
        return 1
    print("ratchets: ok")
    print(health)
    return 0


def starting_baseline(measured: Measurements) -> dict:
    """Every current offender at its size today: how the ratchet was first recorded."""
    allowed = {
        "files": {key: over_limits(values, FILE_LIMITS) for key, values in measured.files.items()},
        "functions": {key: over_limits(values, FUNCTION_LIMITS) for key, values in measured.functions.items()},
        "duplicate_lines_total": sum(measured.duplicate_lines.values()),
    }
    return compare(measured, allowed).baseline


if __name__ == "__main__":
    raise SystemExit(main())
