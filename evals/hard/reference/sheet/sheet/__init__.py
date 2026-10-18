"""Reference spreadsheet engine for the grader's self-test."""

import csv
import io
import math
import re

__all__ = ["Sheet", "SheetError"]

REF = re.compile(r"^([A-Za-z]{1,2})([1-9][0-9]{0,2})$")
ERRORS = ("#DIV/0!", "#REF!", "#CYCLE!", "#NAME?", "#VALUE!", "#ERROR!")


class SheetError(Exception):
    """A spreadsheet error value, such as #DIV/0!."""

    def __init__(self, code):
        super().__init__(code)
        self.code = code


def parse_ref(text):
    """(column, row) for a reference like B12, zero-based; SheetError #REF! otherwise."""
    found = REF.match(text)
    if not found:
        raise SheetError("#REF!")
    letters = found.group(1).upper()
    column = 0
    for letter in letters:
        column = column * 26 + ord(letter) - 64
    return column - 1, int(found.group(2)) - 1


def ref_name(column, row):
    """B12 for (1, 11)."""
    letters = ""
    column += 1
    while column:
        column, rest = divmod(column - 1, 26)
        letters = chr(65 + rest) + letters
    return f"{letters}{row + 1}"


TOKEN = re.compile(r"\s*(?:(\d+\.\d*|\.\d+|\d+)|([A-Za-z]{1,2}[0-9]+(?::[A-Za-z]{1,2}[0-9]+)?)|([A-Za-z_][A-Za-z0-9_]*)|(<=|>=|<>|[-+*/^(),=<>]))")


def tokenize(text):
    tokens, position = [], 0
    text = text.rstrip()
    while position < len(text):
        found = TOKEN.match(text, position)
        if not found:
            raise SheetError("#ERROR!")
        number, ref, name, op = found.groups()
        if number is not None:
            tokens.append(("num", float(number)))
        elif ref is not None:
            tokens.append(("ref", ref.upper()))
        elif name is not None:
            tokens.append(("name", name.upper()))
        else:
            tokens.append(("op", op))
        position = found.end()
    return tokens


class Parser:
    """Recursive descent: comparison < additive < multiplicative < unary < power < atom."""

    def __init__(self, tokens):
        self.tokens, self.position = tokens, 0

    def peek(self):
        return self.tokens[self.position] if self.position < len(self.tokens) else ("end", None)

    def take(self, kind=None, value=None):
        token = self.peek()
        if (kind and token[0] != kind) or (value and token[1] != value):
            raise SheetError("#ERROR!")
        self.position += 1
        return token

    def parse(self):
        tree = self.comparison()
        if self.peek()[0] != "end":
            raise SheetError("#ERROR!")
        return tree

    def comparison(self):
        left = self.additive()
        while self.peek() in [("op", o) for o in ("=", "<>", "<", ">", "<=", ">=")]:
            op = self.take()[1]
            left = ("bin", op, left, self.additive())
        return left

    def additive(self):
        left = self.multiplicative()
        while self.peek() in (("op", "+"), ("op", "-")):
            op = self.take()[1]
            left = ("bin", op, left, self.multiplicative())
        return left

    def multiplicative(self):
        left = self.unary()
        while self.peek() in (("op", "*"), ("op", "/")):
            op = self.take()[1]
            left = ("bin", op, left, self.unary())
        return left

    def unary(self):
        if self.peek() in (("op", "-"), ("op", "+")):
            op = self.take()[1]
            operand = self.unary()
            return ("neg", operand) if op == "-" else operand
        return self.power()

    def power(self):
        base = self.atom()
        if self.peek() == ("op", "^"):
            self.take()
            return ("bin", "^", base, self.unary())
        return base

    def atom(self):
        kind, value = self.peek()
        if kind == "num":
            self.take()
            return ("num", value)
        if kind == "ref":
            self.take()
            return ("range", value) if ":" in value else ("ref", value)
        if kind == "name":
            self.take()
            self.take("op", "(")
            args = []
            if self.peek() != ("op", ")"):
                args.append(self.comparison())
                while self.peek() == ("op", ","):
                    self.take()
                    args.append(self.comparison())
            self.take("op", ")")
            return ("call", value, args)
        if (kind, value) == ("op", "("):
            self.take()
            inner = self.comparison()
            self.take("op", ")")
            return inner
        raise SheetError("#ERROR!")


def number_of(value):
    if value is None:
        return 0.0
    if isinstance(value, str):
        raise SheetError("#VALUE!")
    return value


def display(value):
    """How a computed value is shown."""
    if value is None:
        return ""
    if isinstance(value, SheetError):
        return value.code
    if isinstance(value, str):
        return value
    if math.isclose(value, round(value), abs_tol=1e-9):
        return str(int(round(value)))
    return format(value, ".10g")


class Sheet:
    """Cells by reference; formulas recompute from current inputs on every read."""

    def __init__(self):
        self.raw = {}

    def set(self, ref, text):
        """Sets a cell's text ('' clears it)."""
        key = ref_name(*parse_ref(ref))
        if text == "" or text is None:
            self.raw.pop(key, None)
        else:
            self.raw[key] = str(text)

    def text(self, ref):
        """The cell's raw text."""
        return self.raw.get(ref_name(*parse_ref(ref)), "")

    def get(self, ref):
        """The computed value: None, a float, a string, or a SheetError."""
        key = ref_name(*parse_ref(ref))
        try:
            return self._value(key, ())
        except SheetError as error:
            return error

    def display(self, ref):
        """The computed value as shown."""
        return display(self.get(ref))

    def _value(self, key, stack):
        if key in stack:
            raise SheetError("#CYCLE!")
        text = self.raw.get(key)
        if text is None:
            return None
        if text.startswith("="):
            return self._eval(Parser(tokenize(text[1:])).parse(), stack + (key,))
        try:
            return float(text)
        except ValueError:
            return text

    def _cells(self, span, stack):
        start, end = span.split(":")
        (c1, r1), (c2, r2) = parse_ref(start), parse_ref(end)
        values = []
        for column in range(min(c1, c2), max(c1, c2) + 1):
            for row in range(min(r1, r2), max(r1, r2) + 1):
                values.append(self._value(ref_name(column, row), stack))
        return values

    def _eval(self, node, stack):
        kind = node[0]
        if kind == "num":
            return node[1]
        if kind == "ref":
            return self._value(ref_name(*parse_ref(node[1])), stack)
        if kind == "range":
            raise SheetError("#VALUE!")
        if kind == "neg":
            return -number_of(self._eval(node[1], stack))
        if kind == "bin":
            _, op, left, right = node
            a, b = number_of(self._eval(left, stack)), number_of(self._eval(right, stack))
            if op == "+":
                return a + b
            if op == "-":
                return a - b
            if op == "*":
                return a * b
            if op == "/":
                if b == 0:
                    raise SheetError("#DIV/0!")
                return a / b
            if op == "^":
                try:
                    return float(a ** b)
                except (OverflowError, ZeroDivisionError):
                    raise SheetError("#DIV/0!") from None
            return float({"=": a == b, "<>": a != b, "<": a < b, ">": a > b, "<=": a <= b, ">=": a >= b}[op])
        return self._call(node[1], node[2], stack)

    def _numbers(self, args, stack):
        numbers = []
        for arg in args:
            if arg[0] == "range":
                for value in self._cells(arg[1], stack):
                    if isinstance(value, float):
                        numbers.append(value)
            else:
                numbers.append(number_of(self._eval(arg, stack)))
        return numbers

    def _call(self, name, args, stack):
        if name == "IF":
            if len(args) != 3:
                raise SheetError("#ERROR!")
            condition = number_of(self._eval(args[0], stack))
            return self._eval(args[1] if condition != 0 else args[2], stack)
        if name in ("SUM", "AVG", "MIN", "MAX", "COUNT"):
            numbers = self._numbers(args, stack)
            if name == "SUM":
                return float(sum(numbers))
            if name == "COUNT":
                return float(len(numbers))
            if not numbers:
                raise SheetError("#DIV/0!") if name == "AVG" else SheetError("#VALUE!")
            return {"AVG": sum(numbers) / len(numbers), "MIN": min(numbers), "MAX": max(numbers)}[name]
        if name == "ABS" and len(args) == 1:
            return abs(number_of(self._eval(args[0], stack)))
        if name == "ROUND" and len(args) in (1, 2):
            digits = int(number_of(self._eval(args[1], stack))) if len(args) == 2 else 0
            value = number_of(self._eval(args[0], stack))
            scaled = math.floor(abs(value) * 10 ** digits + 0.5) / 10 ** digits
            return float(math.copysign(scaled, value)) if scaled else 0.0
        raise SheetError("#NAME?")

    @classmethod
    def from_csv(cls, text):
        """A sheet from CSV text: row 1 is A1, B1, ..."""
        sheet = cls()
        for row, values in enumerate(csv.reader(io.StringIO(text))):
            for column, value in enumerate(values):
                if value != "":
                    sheet.set(ref_name(column, row), value)
        return sheet

    def to_csv(self, values=True):
        """CSV of computed values (or raw text), up to the last used row and column."""
        if not self.raw:
            return ""
        spots = [parse_ref(key) for key in self.raw]
        width = max(c for c, _ in spots) + 1
        height = max(r for _, r in spots) + 1
        out = io.StringIO()
        writer = csv.writer(out, lineterminator="\n")
        for row in range(height):
            cells = [ref_name(column, row) for column in range(width)]
            writer.writerow([self.display(c) if values else self.raw.get(c, "") for c in cells])
        return out.getvalue()

