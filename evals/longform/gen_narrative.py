"""Narrative long-form cases: a chronicle, an epistolary set and a meeting transcript."""

import re

from .kit import MONTHS, PLACES, Case, anchored, ordinal, people

LANDMARKS = ["old well", "cracked bell tower", "salt warehouse", "toll bridge", "chapel steps",
             "tide gate", "market cross", "lime kiln", "reed jetty", "orchard wall"]
MOODS = ["wary", "restless", "quietly amused", "bone-tired", "stubborn", "hopeful", "uneasy"]
WEATHER = ["a thin rain", "low fog", "a dry north wind", "sleet", "heavy cloud", "pale sun"]
FILLER = [
    "{A} walked the length of {P} in {W}, saying little and watching the roofs.",
    "By evening {A} was {M}, and {B} noticed it without comment.",
    "Nobody in {P} spoke of the matter openly, though {A} and {B} both knew it was coming.",
    "{B} kept the shutters closed against {W} and listened to the street.",
    "At the {L} in {P}, {A} waited while {B} argued with a stranger about the road.",
    "The talk at {P} turned, as it always did, to the harbour dues and to who had paid them.",
    "{A} had learned long ago that a {M} silence is worth more than a promise.",
    "It was {W} when {B} finally rode out of {P}, and {A} did not wave.",
    "The {L} at {P} had stood longer than anyone could remember, and {A} leaned on it to think.",
    "{B} wrote nothing down that night; {A} wrote a great deal and burned most of it.",
    "Rumour reached {P} before the riders did, and {A} was already {M} when it arrived.",
    "They shared bread at the {L}, {A} and {B}, and neither mentioned the debt.",
    "The magistrate of {P} sent word twice and was ignored twice, to {A}'s private satisfaction.",
    "{B} counted the days by the tolling of the bell, and {A} counted them by the dwindling candles.",
]


def _filler(rng, cast, places, sentences):
    out = []
    for _ in range(sentences):
        a, b = rng.sample(cast, 2)
        out.append(rng.choice(FILLER).format(
            A=a.split()[0], B=b.split()[0], P=rng.choice(places), W=rng.choice(WEATHER),
            M=rng.choice(MOODS), L=rng.choice(LANDMARKS)))
    return " ".join(out)


def chronicle(root):
    case = Case("longform-chronicle", "narrative", 71,
                "Twelve-chapter synthetic chronicle (~100 KB) with a large cast, places, objects, dates and quotes.")
    rng, uniq, t = case.rng, case.unique, case.t
    cast = people(rng, 24)
    places = rng.sample(PLACES, 14)
    day = 3  # running day count across the story; each event gets a strictly later date
    facts = {}  # (chapter, kind) -> dict

    def when():
        nonlocal day
        day += rng.randint(2, 6)
        year = 1403 + (day - 1) // 252
        d = (day - 1) % 252
        return f"{ordinal(d % 28 + 1)} of {MONTHS[d // 28]}, {year}"

    for chapter in range(1, 13):
        events = []
        for _ in range(5):
            a, b = rng.sample(cast, 2)
            place = rng.choice(places)
            kind = rng.choice(["gift", "count", "ship", "leave", "quote"])
            date = when()
            if kind == "gift":
                obj = uniq.obj()
                text = (f"On the {date}, {a} pressed a {obj} into the hands of {b} beneath the "
                        f"{rng.choice(LANDMARKS)} at {place}, and asked that no one be told.")
                info = dict(a=a, b=b, place=place, answer=obj,
                            q=f"In the Chronicle, which object did {a} give to {b} at {place}?")
            elif kind == "count":
                n = uniq.number(2100, 9800)
                goods = rng.choice(["barrels", "bales", "lanterns", "sacks", "crates"])
                text = (f"On the {date}, {a} counted {n} {goods} stacked in the {rng.choice(LANDMARKS)} "
                        f"at {place} and entered the figure in the ledger of {b}.")
                info = dict(a=a, b=b, place=place, answer=n,
                            q=f"How many {goods} did {a} count at {place} on the {date}?")
            elif kind == "ship":
                ship = uniq.ship()
                text = (f"{a} sailed into {place} aboard the {ship} on the {date}, "
                        f"carrying a message for {b}.")
                info = dict(a=a, b=b, place=place, answer=ship,
                            q=f"Which vessel carried {a} into {place} on the {date}?")
            elif kind == "leave":
                text = f"{a} quit {place} before dawn on the {date}, leaving {b} to explain."
                info = dict(a=a, b=b, place=place, answer=date,
                            q=f"On what date did {a} quit {place}, leaving {b} to explain?")
            else:
                phrase = uniq.phrase(6)
                phrase = phrase[0].upper() + phrase[1:]
                text = f"At {place} on the {date}, {a} told {b}, \"{phrase}.\""
                info = dict(a=a, b=b, place=place, answer=phrase, quote=True,
                            q=f"What exactly did {a} tell {b} at {place} on the {date}? Quote the sentence verbatim.")
            info["text"], info["kind"] = text, kind
            events.append(info)
        paragraphs = []
        for event in events:
            paragraphs.append(_filler(rng, cast, places, rng.randint(9, 12)))
            paragraphs.append(event["text"])
            paragraphs.append(_filler(rng, cast, places, rng.randint(10, 14)))
        title = f"Chapter {chapter}: The {rng.choice(['Quiet','Long','Thin','Last','Grey'])} {rng.choice(['Tide','Road','Ledger','Winter','Gate'])}"
        body = f"{title}\n\n" + "\n\n".join(paragraphs)
        record = t.user(f"Attachment chronicle-ch{chapter:02d}.txt (chapter {chapter} of 12 of the Saltmere Chronicle). "
                        f"Read it carefully; I will ask detailed questions later.\n\n{body}")
        t.asst(f"Received chapter {chapter} of 12. I will keep its details available for your questions.")
        for event in events:
            event["record"], event["chapter"] = record, chapter
            facts.setdefault(chapter, []).append(event)
        if chapter % 2 == 0:
            t.boundary()

    # Probes: spread over the chronicle; chapter 11-12 facts sit in the verbatim tail and are excluded later.
    picks = [(1, 0), (1, 3), (2, 1), (3, 2), (4, 0), (5, 3), (6, 1), (7, 4), (8, 2), (9, 0), (10, 3), (11, 1)]
    detail = quotes = 0
    for chapter, index in picks:
        event = facts[chapter][index]
        if event.get("quote"):
            if quotes < 2:
                case.probe("quote", event["q"], event["record"], exact=event["answer"])
                quotes += 1
            continue
        if detail < 9:
            case.probe("detail", event["q"], event["record"], answer=event["answer"])
            detail += 1
    for chapter in (2, 5, 6, 8):  # top up quotes/details if the random mix fell short
        for event in facts[chapter]:
            if event.get("quote") and quotes < 2 and not any(p["fact_records"] == [event["record"]] and p["category"] == "quote" for p in case.probes):
                case.probe("quote", event["q"], event["record"], exact=event["answer"])
                quotes += 1
            elif not event.get("quote") and detail < 9 and not any(p.get("expected", {}).get("regex") == anchored(event["answer"]) for p in case.probes):
                case.probe("detail", event["q"], event["record"], answer=event["answer"])
                detail += 1
    case.probe("decoy", "What was the name of the ferryman's dog in the Chronicle?", [],
               absent=[r"\bdog\b", r"\bhound\b", r"\bferryman\b"])
    case.probe("decoy", "How much did the tavern at Saltmere charge for a night's lodging?", [],
               absent=[r"\blodging\b", r"\btavern\b", r"\binn\b"])
    case.finish(root, t.boundaries + ([len(t.records)] if t.boundaries[-1] != len(t.records) else []))
    return case


# ---------------------------------------------------------------- epistolary letters

def letters(root):
    case = Case("longform-letters", "narrative", 72,
                "Sixty-odd synthetic letters among nine correspondents, with amounts, appointments, quotes and retractions.")
    rng, uniq, t = case.rng, case.unique, case.t
    cast = people(rng, 9)
    places = rng.sample(PLACES, 10)
    day = 5
    letters_out = []

    def when():
        nonlocal day
        day += rng.randint(1, 4)
        d = (day - 1) % 252
        return f"{ordinal(d % 28 + 1)} of {MONTHS[d // 28]}, {1411 + (day - 1) // 252}"

    OPEN = ["I trust this finds you in better spirits than the weather allows.",
            "Forgive the haste of this note; the post leaves within the hour.",
            "You will have heard the talk in town by now, and I will not repeat it.",
            "I write by a poor candle, so you must excuse the hand.",
            "Nothing here has changed, except that everything has."]
    CLOSE = ["Burn this when you have read it.", "Give my regards to the household.",
             "Write soon, and write plainly.", "I remain, as ever, your correspondent."]
    facts = []

    def letter(sender, recipient, body, subject):
        date = when()
        n = len(letters_out) + 1
        text = (f"Letter {n}\nDate: {date}\nFrom: {sender}\nTo: {recipient}\nSubject: {subject}\n\n"
                f"{rng.choice(OPEN)} {body} {_filler(rng, cast, places, 6)} {rng.choice(CLOSE)}\n-- {sender.split()[0]}")
        letters_out.append(text)
        return n, date

    pending = []  # (sender, recipient, kind, data) to retract later
    for n in range(66):
        sender, recipient = rng.sample(cast, 2)
        kind = rng.choice(["debt", "meet", "gift", "quote", "debt", "meet"])
        if kind == "debt":
            obj, sum1 = uniq.obj(), uniq.number(120, 980)
            body = f"I confirm that I owe you {sum1} guineas for the {obj} you left with me."
            idx, date = letter(sender, recipient, body, f"Account of the {obj}")
            facts.append(dict(kind="debt", idx=idx, date=date, s=sender, r=recipient, obj=obj, amount=sum1))
            pending.append(facts[-1])
        elif kind == "meet":
            place, date2, obj = rng.choice(places), when(), uniq.obj()
            body = f"I shall wait for you at {place} on the {date2}; do bring the {obj}, and nothing else."
            idx, date = letter(sender, recipient, body, f"Appointment at {place}")
            facts.append(dict(kind="meet", idx=idx, date=date, s=sender, r=recipient, place=place, when=date2, obj=obj))
        elif kind == "gift":
            obj = uniq.obj()
            body = f"I am sending you my {obj} by the next carrier, and I will not accept payment."
            idx, date = letter(sender, recipient, body, "A small parcel")
            facts.append(dict(kind="gift", idx=idx, date=date, s=sender, r=recipient, obj=obj))
        else:
            phrase = uniq.phrase(6)
            phrase = phrase[0].upper() + phrase[1:]
            body = f"Let me put it as plainly as I can: \"{phrase}.\""
            idx, date = letter(sender, recipient, body, "On the matter between us")
            facts.append(dict(kind="quote", idx=idx, date=date, s=sender, r=recipient, phrase=phrase))
        # An earlier debt gets retracted by its own sender a few letters later.
        if n % 9 == 8 and pending:
            old = pending.pop(0)
            new = uniq.number(120, 980)
            body = (f"Pray disregard the sum I gave in my letter of the {old['date']} about the {old['obj']}: "
                    f"I miscounted, and the correct amount owed is {new} guineas.")
            idx, date = letter(old["s"], old["r"], body, f"Correction regarding the {old['obj']}")
            old.update(retract_idx=idx, new=new, retract_date=date)
    # Bundle letters into user messages of ~11 letters.
    owner = {}
    for start in range(0, len(letters_out), 11):
        chunk = letters_out[start:start + 11]
        record = t.user(f"Here is the next bundle of letters from the Marlowe papers (letters {start + 1}-{start + len(chunk)}). "
                        "Please keep every sender, recipient, date, sum and quotation straight.\n\n" + "\n\n".join(chunk))
        t.asst(f"Filed letters {start + 1}-{start + len(chunk)}. I will answer questions on them later.")
        for k in range(start + 1, start + len(chunk) + 1):
            owner[k] = record
        if (start // 11) % 1 == 0 and start // 11 >= 1 and start // 11 % 1 == 0:
            t.boundary()
    debts = [f for f in facts if f["kind"] == "debt" and "new" in f]
    plain_debts = [f for f in facts if f["kind"] == "debt" and "new" not in f]
    meets = [f for f in facts if f["kind"] == "meet"]
    gifts = [f for f in facts if f["kind"] == "gift"]
    quotes = [f for f in facts if f["kind"] == "quote"]

    def header(f):
        return rf"Date:\s*{re.escape(f['date'])}\s*\nFrom:\s*{re.escape(f['s'])}\s*\nTo:\s*{re.escape(f['r'])}"

    # Detail probes (9): corrected sums (stale = the retracted sum), attribution, appointments, gifts.
    for f in _spread(debts, 3):
        case.probe("retraction", f"After the later correction, how many guineas did the writer finally say was owed for the {f['obj']}?",
                   owner[f["retract_idx"]], answer=str(f["new"]), stale=[str(f["amount"])])
    for f in _spread(plain_debts, 1):
        case.probe("detail", f"How many guineas did {f['s']} say was owed to {f['r']} for the {f['obj']}?",
                   owner[f["idx"]], answer=str(f["amount"]))
    for f in _spread(meets, 3):
        case.probe("detail", f"Which item did {f['s']} ask {f['r']} to bring to the meeting at {f['place']} on the {f['when']}?",
                   owner[f["idx"]], answer=f["obj"])
    for f in _spread(gifts, 2):
        case.probe("attribution", f"Who sent the {f['obj']} to whom? Answer in the form 'Sender -> Recipient'.",
                   owner[f["idx"]],
                   regex=anchored(f["s"]) + r"\s*(?:->|→|to)\s*" + anchored(f["r"]), text=f"{f['s']} -> {f['r']}",
                   evidence=r"From:\s*" + re.escape(f["s"]) + r"\s*\nTo:\s*" + re.escape(f["r"]) + r"(?:(?!Letter \d).)*" + re.escape(f["obj"]))
    for f in _spread(quotes, 2):
        case.probe("quote", f"What exact sentence did {f['s']} put in quotation marks in the letter dated the {f['date']}? Quote it verbatim.",
                   owner[f["idx"]], exact=f["phrase"])
    case.probe("decoy", "What was the name of the vessel on which the Marlowe letters were shipped abroad?", [],
               absent=[r"\bshipped abroad\b", r"\bvessel\b", r"\bfreighter\b"])
    case.probe("decoy", "How many pages did the last letter in the Marlowe papers run to?", [],
               absent=[r"\bpages\b", r"\bfolios\b"])
    case.finish(root, _ends(t))
    return case


def _spread(items, count):
    """`count` items spread evenly over the list (so probes cover early, middle and late material)."""
    if len(items) <= count:
        return list(items)
    return [items[round(i * (len(items) - 1) / max(count - 1, 1))] for i in range(count)] if count > 1 else [items[len(items) // 3]]


def _ends(t):
    ends = list(t.boundaries)
    if not ends or ends[-1] != len(t.records):
        ends.append(len(t.records))
    return ends


# ---------------------------------------------------------------- meeting transcript

def meeting(root):
    case = Case("longform-meeting", "narrative", 73,
                "Three-hour synthetic steering-committee transcript with speaker attribution and reversed decisions.")
    rng, uniq, t = case.rng, case.unique, case.t
    cast = people(rng, 7)
    clock = [9 * 3600]
    topics = ["harbour dredging", "the ferry timetable", "the grain levy", "the lighthouse repairs",
              "school funding", "the market charter", "the bridge toll", "the winter fuel fund"]
    CHAT = ["I think we are all agreed on the general direction, but the numbers worry me.",
            "Can we hear from the treasurer before anyone else speaks again?",
            "That was tried two years ago and it did not hold.",
            "I would like that recorded in the minutes, please.",
            "We keep circling the same point; let us move on and come back to it.",
            "The clerk has the figures, and the figures are not in dispute.",
            "My ward will not accept that, and I will say so plainly.",
            "Let me put the objection another way, for the benefit of those who missed it.",
            "There is a procedural question here that nobody has answered yet.",
            "I defer to the chair on this, though not happily."]

    def say(speaker, text):
        clock[0] += rng.randint(8, 70)
        h, rem = divmod(clock[0], 3600)
        m, s = divmod(rem, 60)
        return f"[{h:02d}:{m:02d}:{s:02d}] {speaker}: {text}"

    sessions = []
    facts = []
    for topic_index, topic in enumerate(topics):
        lines = [f"== Agenda item {topic_index + 1}: {topic} =="]
        chair, proposer, objector = rng.sample(cast, 3)
        budget1, budget2 = uniq.number(20, 98), uniq.number(120, 480)
        date1, date2 = uniq.date(1412), uniq.date(1412)
        for _ in range(rng.randint(14, 18)):
            lines.append(say(rng.choice(cast), rng.choice(CHAT) + " " + _filler(rng, cast, PLACES, 2)))
        lines.append(say(proposer, f"I propose we fund {topic} at {budget1} thousand crowns, with work starting on the {date1}."))
        lines.append(say(chair, f"The chair records the decision: {topic} is funded at {budget1} thousand crowns, starting the {date1}."))
        for _ in range(rng.randint(16, 22)):
            lines.append(say(rng.choice(cast), rng.choice(CHAT) + " " + _filler(rng, cast, PLACES, 2)))
        phrase = uniq.phrase(6)
        phrase = phrase[0].upper() + phrase[1:]
        lines.append(say(objector, f"I object to the schedule, and I will say it once: \"{phrase}.\""))
        lines.append(say(objector, f"New figures: {topic} should cost {budget2} thousand crowns and start on the {date2}."))
        lines.append(say(chair, f"The chair revises the earlier decision: {topic} is now funded at {budget2} thousand crowns, starting the {date2}. The {budget1} thousand figure is withdrawn."))
        for _ in range(rng.randint(10, 14)):
            lines.append(say(rng.choice(cast), rng.choice(CHAT) + " " + _filler(rng, cast, PLACES, 2)))
        sessions.append("\n".join(lines))
        facts.append(dict(topic=topic, chair=chair, proposer=proposer, objector=objector,
                          b1=budget1, b2=budget2, d1=date1, d2=date2, phrase=phrase))
    owner = []
    for i, text in enumerate(sessions):
        record = t.user(f"Transcript segment {i + 1} of {len(sessions)} from the steering committee (verbatim, timestamps included). "
                        f"Keep speakers and every decision straight.\n\n{text}")
        t.asst(f"Segment {i + 1} noted. I will track who said what and which decisions changed.")
        owner.append(record)
        if i % 2 == 1:
            t.boundary()
    for i in (0, 1, 3):
        f = facts[i]
        case.probe("retraction", f"What is the final agreed funding for {f['topic']} after the chair's revision (in thousand crowns)?",
                   owner[i], answer=f["b2"], stale=[f["b1"]],
                   evidence=rf"revises the earlier decision: {re.escape(f['topic'])} is now funded at {f['b2']}")
    for i in (0, 2, 5):
        f = facts[i]
        case.probe("detail", f"What start date was first recorded by the chair for {f['topic']} before it was revised?",
                   owner[i], answer=f["d1"], evidence=rf"chair records the decision: {re.escape(f['topic'])}[^\n]*{anchored(f['d1'])}")
    for i in (1, 3, 5):
        f = facts[i]
        case.probe("attribution", f"Who proposed the original funding for {f['topic']}?",
                   owner[i], regex=anchored(f["proposer"]), text=f["proposer"],
                   evidence=rf"{re.escape(f['proposer'])}: I propose we fund {re.escape(f['topic'])}")
    for i in (2, 4):
        f = facts[i]
        case.probe("quote", f"What exactly did {f['objector']} say when objecting to the {f['topic']} schedule? Quote the sentence verbatim.",
                   owner[i], exact=f["phrase"])
    case.probe("decoy", "What did the committee decide about the museum roof?", [],
               absent=[r"\bmuseum\b", r"\broof\b"])
    case.probe("decoy", "Which member abstained from the vote on the grain levy?", [],
               absent=[r"\babstain", r"\bvote\b", r"\bballot\b"])
    case.finish(root, _ends(t))
    return case
