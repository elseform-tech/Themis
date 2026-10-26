"""Dates and times.

Invoice dates are calendar dates in UTC; timestamps are timezone-aware UTC datetimes
(docs/decisions/0003-dates.md). Stored timestamps written by Ledgerly 0.7 and earlier have no
offset: they were always UTC.
"""

from datetime import date, datetime, timedelta, timezone


def utc_now():
    """The current time as an aware UTC datetime."""
# >>> plant:lookup-deprecation
    return datetime.utcnow()
# === plant:lookup-deprecation
    return datetime.now(timezone.utc)
# <<< plant:lookup-deprecation


def today():
    return utc_now().date()


def parse_date(text):
    """A calendar date from "YYYY-MM-DD", "DD/MM/YYYY" or an ISO timestamp with an offset
    ("2026-03-04T23:30:00-05:00"), which counts as its UTC date."""
    text = str(text).strip()
# >>> plant:run-to-see
# === plant:run-to-see
    if "T" in text:
        moment = datetime.fromisoformat(text)
        if moment.tzinfo is None:
            raise ValueError(f"timestamp without an offset: {text!r}")
        return moment.astimezone(timezone.utc).date()
# <<< plant:run-to-see
    if "/" in text:
        day, month, year = (int(part) for part in text.split("/"))
        return date(year, month, day)
    return date.fromisoformat(text)


def parse_moment(text):
    """A stored timestamp as an aware UTC datetime (offset-less ones are UTC)."""
    moment = datetime.fromisoformat(text)
# >>> plant:lookup-deprecation
    return moment
# === plant:lookup-deprecation
    return moment if moment.tzinfo else moment.replace(tzinfo=timezone.utc)
# <<< plant:lookup-deprecation


def month_key(day):
    """2026-03-04 -> "2026-03"."""
    return f"{day.year:04d}-{day.month:02d}"


def add_days(day, days):
    return day + timedelta(days=days)


def is_overdue(due, now=None):
    """Whether a due date has passed (the due day itself is not overdue)."""
    now = now or utc_now()
    return due < now.date()
