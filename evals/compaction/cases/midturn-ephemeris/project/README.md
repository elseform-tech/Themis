# Planetarium ephemeris

Write three files, plain JavaScript ES modules, no dependencies:

1. `src/astro/vsop87.js`: `parseVsop87(text)` returns `{L: [[A,B,C],...] per power, B: ..., R: ...}` and `evaluate(series, T)`; the file format is described in `docs/vsop87.txt`.
2. `src/astro/ephemeris.js`: `planetRaDec(earthText, planetText, jdTT)` returns geocentric apparent `{ra, dec}` in degrees, using `src/astro/time.js` for time arguments.
3. `tests/ephemeris.test.js` (node:test) comparing Mars with `data/horizons_reference.json` within 1 arcminute, in the style of `tests/time.test.js`.
