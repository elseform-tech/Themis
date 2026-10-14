"""Shared helpers for the synthetic long-form fixtures (names, unique tokens, probe bookkeeping)."""

import json
import os
import random
import re

from evals.compaction.fixturekit import Transcript

FIRST = ("Oriel Brannoch Ysolde Tavish Marit Corvin Leofric Imara Dunstan Perrin Halwen Sorrel "
         "Gideon Aurelia Fenwick Lysander Odalys Ruadh Thessaly Wystan Cassia Bertrand Ilse "
         "Jorund Katell Lucan Maren Nerys Osric Petra Quillon Rowena Silas Teodor Una Valdis "
         "Wilmot Xanthe Yarrow Zephra Anselm Briony Caldwell Delphine Eamon Faolan Gwendal "
         "Hestia Ivor Jessamy Kestrel Leontine Merrick Nuala Ottoline Prosper Rosalind Sigrid "
         "Tobias Ulrika Vesper").split()
LAST = ("Vance Harrowgate Pellam Ashdown Thornwick Marlowe Greaves Quillfeather Stannard "
        "Orlebar Wexcombe Fairweather Lindqvist Ballantyne Cresswell Dunmore Elsworth "
        "Fotheringay Garrick Holloway Ingleby Jessop Kettleby Lockhart Mortimer Nankivell "
        "Overton Penhallow Radcliffe Sedgwick Trevelyan Underhill Valmont Whitcombe Yardley "
        "Zouche Abernathy Blackmoor Carrow Dewhurst Everard Fenner Gallagher Hartwell").split()
PLACES = ("Saltmere Harrowgate Brindle-on-Weir Cinderford Dunlow Eastwick Fenhallow Greywater "
          "Highmoor Ironbridge Kestrelsea Lowmarsh Millbrook Northcarr Oakhollow Pennyfold "
          "Quarrydown Ravensholt Stonewick Tidemouth Underhaven Vellacott Westerly Yarrowmere "
          "Ashgrove Bracken-under-Fell Coldharbour Dovecote Elmstead Foxlea").split()
MONTHS = "Frostmoon Thawmoon Seedmoon Rainmoon Sunmoon Harvestmoon Embermoon Mistmoon Snowmoon".split()
ADJECTIVES = ("brass copper silver tin walnut ivory slate amber jade oaken velvet linen "
              "cracked gilded rusted woven carved painted folded sealed hollow tarnished "
              "polished braided etched dented lacquered stitched enamelled pewter marble").split()
NOUNS = ("astrolabe lantern compass thimble ledger chalice bellows sextant satchel kettle "
         "locket spyglass quill gauntlet hourglass flute sundial brooch tankard bridle "
         "casket cudgel gavel hatchet inkwell knapsack mallet nutmeg-grater oilcan "
         "pocketwatch rattle shuttle spindle trowel tuning-fork whistle yoke zither "
         "abacus bugle candelabrum dagger easel flask goblet harp icon jar").split()
SHIPS = ("Gannet Merrow Halcyon Plover Corbie Skerry Tern Wherry Marlin Curlew Osprey Petrel "
         "Fulmar Kittiwake Shearwater Cormorant Dunlin Godwit Lapwing Sanderling").split()
ROMAN = "I II III IV V VI VII VIII IX X XI XII".split()


def ordinal(n):
    suffix = "th" if 10 <= n % 100 <= 20 else {1: "st", 2: "nd", 3: "rd"}.get(n % 10, "th")
    return f"{n}{suffix}"


class Uniques:
    """Hands out tokens that are never reused, so each probe answer is a unique string."""

    def __init__(self, rng):
        self.rng = rng
        self.used = set()

    def take(self, make):
        while True:
            token = make()
            if token.lower() not in self.used:
                self.used.add(token.lower())
                return token

    def obj(self):
        return self.take(lambda: f"{self.rng.choice(ADJECTIVES)} {self.rng.choice(NOUNS)}")

    def number(self, low=200, high=9800):
        return self.take(lambda: str(self.rng.randint(low, high)))

    def code(self, prefix, digits=4):
        return self.take(lambda: f"{prefix}{self.rng.randint(10 ** (digits - 1), 10 ** digits - 1)}")

    def ship(self):
        return self.take(lambda: f"{self.rng.choice(SHIPS)} of {self.rng.choice(PLACES)}")

    def date(self, year):
        return self.take(lambda: f"{ordinal(self.rng.randint(1, 28))} of {self.rng.choice(MONTHS)}, {year}")

    def phrase(self, words=7):
        bank = (ADJECTIVES + NOUNS + SHIPS)
        return self.take(lambda: " ".join(w.lower() for w in self.rng.sample(bank, words)))


def anchored(text):
    """Case-insensitive regex that matches `text` as a whole token run (not inside longer words/numbers)."""
    body = r"\s+".join(re.escape(part) for part in text.split())
    return rf"(?<![\w.-]){body}(?![\w-]|\.\d)"


def people(rng, count):
    names = set()
    while len(names) < count:
        names.add(f"{rng.choice(FIRST)} {rng.choice(LAST)}")
    return sorted(names)


class Case:
    """Transcript plus planted-fact probes for one long-form case."""

    def __init__(self, name, kind, seed, description):
        self.name, self.kind, self.description = name, kind, description
        self.rng = random.Random(seed)
        self.t = Transcript(seed)
        self.t.rng = self.rng
        self.unique = Uniques(self.rng)
        self.probes = []

    def probe(self, category, question, record, answer=None, regex=None, evidence=None, exact=None,
              stale=None, absent=None, text=None):
        """Registers a probe. `record` is the 1-based record number (or list) stating the fact.

        `answer` is the unique string the answer must contain (anchored regex), `regex` an explicit
        replacement. `evidence` is the regex that identifies the original text stating the fact; it
        drives the in-context exclusion and citation validation, and defaults to the answer pattern
        (set it when the answer, such as a person's name, appears all over the material). `exact`
        demands the verbatim quote. `absent` (decoys) lists patterns that must never occur in the
        transcript.
        """
        records = [record] if isinstance(record, int) else list(record or [])
        probe = {"id": f"{self.name}-{len(self.probes) + 1:02d}", "category": category,
                 "question": question, "fact_records": records}
        if category == "decoy":
            probe.update({"expect_unknown": True, "absent": absent})
        else:
            probe["expected"] = {"regex": regex or anchored(answer or exact)}
            if exact:
                probe["exact"] = exact
                probe["expected"] = {"regex": anchored(exact)}
            probe["evidence"] = evidence or probe["expected"]["regex"]
            # A literal correct answer, used only by offline scripted providers and docs.
            probe["answer_text"] = text or exact or answer
            assert probe["answer_text"], probe["id"]
            if stale:
                probe["stale"] = [anchored(s) for s in stale]
        self.probes.append(probe)
        return probe

    def finish(self, root, chunk_ends, next_action=""):
        """Validates the planted facts against the transcript and writes the case files."""
        t, total = self.t, len(self.t.records)
        texts = [json.dumps(r, ensure_ascii=False) for r in t.records]
        decoded = [_flatten(r) for r in t.records]
        assert chunk_ends == sorted(set(chunk_ends)) and chunk_ends[-1] == total, (self.name, chunk_ends)
        assert len(chunk_ends) >= 3, f"{self.name}: needs at least three compactions"
        for probe in self.probes:
            if probe["category"] == "decoy":
                for pattern in probe["absent"]:
                    assert not any(re.search(pattern, text, re.I) for text in texts), (probe["id"], pattern)
                continue
            hits = [i + 1 for i, text in enumerate(decoded)
                    if re.search(probe["evidence"], text, re.I | re.S)]
            planted = set(probe["fact_records"])
            # A tool call's arguments may repeat the fact stated in its result (the next record).
            extra = {h for h in hits if h not in planted and h + 1 in planted
                     and t.records[h - 1].get("tool_calls")}
            assert planted and planted <= set(hits) and set(hits) - extra == planted, (
                probe["id"], hits, probe["fact_records"])
            for pattern in probe.get("stale", []):
                assert any(re.search(pattern, text, re.I) for text in decoded), (probe["id"], pattern)
        os.makedirs(root, exist_ok=True)
        t.write(os.path.join(root, "transcript.jsonl"))
        with open(os.path.join(root, "probes.json"), "w", encoding="utf-8") as handle:
            json.dump(self.probes, handle, indent=2, ensure_ascii=False)
            handle.write("\n")
        size = sum(len(text) for text in decoded)
        with open(os.path.join(root, "case.json"), "w", encoding="utf-8") as handle:
            json.dump({"name": self.name, "kind": self.kind, "description": self.description,
                       "suites": ["longform"], "records": total, "chunks": chunk_ends,
                       "material_bytes": size, "next_action": next_action},
                      handle, indent=2, ensure_ascii=False)
            handle.write("\n")


def _flatten(record):
    """All string content of a transcript record, joined (decoded, not JSON-escaped)."""
    parts = []

    def walk(value):
        if isinstance(value, str):
            parts.append(value)
        elif isinstance(value, dict):
            for item in value.values():
                walk(item)
        elif isinstance(value, list):
            for item in value:
                walk(item)

    walk(record)
    return "\n".join(parts)


def flatten(record):
    return _flatten(record)
