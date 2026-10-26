# timeparse

`parse_duration(text)` returns the seconds in a duration such as `1h30m`, `45s`, `90m` or
`1h 5m 3s`.

- Units are `h`, `m` and `s`, in that order, each at most once; letters may be upper or lower case.
- Each unit follows a whole number of zero or more digits; spaces may separate the parts and
  surround the whole text.
- Anything else raises `ValueError`: an empty text, an unknown unit, a repeated or out-of-order
  unit, a number without a unit, a unit without a number, a sign or a decimal point.

Run the tests with `python3 -m pytest -q`.
