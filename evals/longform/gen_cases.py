"""Generates the committed long-form fixtures: `python3 -m evals.longform.gen_cases`."""

import os

from . import gen_code, gen_narrative

CASES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "cases")
BUILDERS = [gen_narrative.chronicle, gen_narrative.letters, gen_narrative.meeting,
            gen_code.api_spec, gen_code.build_log, gen_code.refactor]


def main():
    for build in BUILDERS:
        name = build.__name__
        case = build(os.path.join(CASES, name.replace("_", "-")))
        print(f"{case.name}: {len(case.t.records)} records, {len(case.probes)} probes")


if __name__ == "__main__":
    main()
