# iniconf (eval fixture)

`iniconf/__init__.py` is Python 3.14.6's `Lib/configparser.py` and `tests/test_iniconf.py` its
`Lib/test/test_configparser.py`, with `tests/configdata/` from `Lib/test/configdata/`, under the
Python Software Foundation License in `LICENSE.txt`. Changes for the `codebase` eval task:

- the module is a package named `iniconf`, and the tests import it as `configparser`;
- the tests no longer use CPython's `test.support` (some distributions remove it): data files are
  found next to the tests, and `FakePath`, `TESTFN` and the `__all__` check are small local
  versions;
- five exact counts of `converters` are "at least" counts, so a correct new `get*` method (the
  task asks for `getlist`) does not fail them; the tests still check each converter by name;
- two bugs are planted in the module, for the agent to find: `%%` interpolates as `%%`, and
  `vars=` keys are not case-folded. `evals/hard/reference/iniconf/` is the fixed module with the
  requested `getlist`, for the grader's self-tests.

Only `iniconf/` and `tests/` are copied into the agent's project.
