"""Model prices and windows from models.dev, the preflight check, and spend control.

Copied from `evals/budget/spend.py` (shared by the live evals). Cheap models only: a model whose
input price is above `MAX_INPUT_PRICE` is refused before a run starts, and every run and round
has a hard cost cap (`MAX_RUN_COST`, `MAX_ROUND_COST` by default).

Prices are models.dev's `cost` for provider `opencode-go`, in US dollars per million tokens
(`input`, `output`, and `cache_read` when the provider discounts cached input). Every cost here
is an estimate (≈) from the usage the provider reports; reasoning tokens are part of the output
tokens, as OpenAI-compatible usage counts them.
"""

import dataclasses
import json
import os
import re
import threading
import urllib.error
import urllib.request

CATALOG_URL = "https://models.dev/api.json"
PROVIDER = "opencode-go"
GO_BASE_URL = "https://opencode.ai/inference/go/v1"
# Tokens of input a run is assumed to send per token of material, before any run of that size
# has finished: a deliberately high guess, so the first runs of a round are not under-estimated.
PRIOR_INPUT_PER_MATERIAL = 20
PRIOR_OUTPUT_PER_MATERIAL = 1
# US dollars per million input tokens: dearer models are refused.
MAX_INPUT_PRICE = 0.20
MAX_RUN_COST = 1.50
MAX_ROUND_COST = 5.00
DEFAULT_MODEL = "muse-spark-1.3-contributor"


@dataclasses.dataclass(frozen=True)
class Price:
    input: float
    output: float
    cache_read: float | None = None
    # Writing the provider's prompt cache, when it costs more than plain input.
    cache_write: float | None = None

    @property
    def free(self):
        return not (self.input or self.output or self.cache_read or self.cache_write)


@dataclasses.dataclass(frozen=True)
class ModelInfo:
    model: str
    price: Price
    window: int | None
    # (context size, price) for requests whose context is above that size, smallest first.
    tiers: tuple = ()

    def price_at(self, budget):
        """The price of a run at a budget: the highest tier its requests can reach. Usage is
        totalled per run, so a tiered run is priced as if every request were in that tier (an
        upper bound)."""
        price = self.price
        for size, tier in self.tiers:
            if budget > size:
                price = tier
        return price


def load_catalog(source=None):
    """models.dev's catalog from a URL (default) or a local JSON file (tests, offline use)."""
    source = source or CATALOG_URL
    if os.path.isfile(source):
        with open(source, encoding="utf-8") as handle:
            return json.load(handle)
    request = urllib.request.Request(source, headers={"User-Agent": "themis-evals"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def _price(cost):
    def number(key):
        return None if cost.get(key) is None else float(cost[key])
    return Price(float(cost.get("input") or 0), float(cost.get("output") or 0), number("cache_read"),
                 number("cache_write"))


def model_info(catalog, model):
    """The model's price, context-size tiers and window, or None when the catalog does not list
    it. Tiers come from `cost.tiers` (`tier.type == "context"`) and `cost.context_over_200k`."""
    entry = ((catalog.get(PROVIDER) or {}).get("models") or {}).get(model)
    if entry is None:
        return None
    cost = entry.get("cost") or {}
    tiers = {}
    for tier in cost.get("tiers") or []:
        if (tier.get("tier") or {}).get("type") == "context" and tier["tier"].get("size"):
            tiers[int(tier["tier"]["size"])] = _price(tier)
    if cost.get("context_over_200k"):
        tiers.setdefault(200_000, _price(cost["context_over_200k"]))
    return ModelInfo(model, _price(cost), (entry.get("limit") or {}).get("context"),
                     tuple(sorted(tiers.items())))


def clamp(budget, window):
    return min(budget, window) if window else budget


def cost(price, usage):
    """≈ US dollars for a run's usage: {input_tokens, cached_input_tokens, output_tokens,
    estimated_input_tokens, requests_missing_input_usage}. Requests the provider reported no
    input for are covered by the byte estimate of every request's input."""
    reported = usage.get("input_tokens") or 0
    if usage.get("requests_missing_input_usage"):
        reported = max(reported, usage.get("estimated_input_tokens") or 0)
    cached = min(usage.get("cached_input_tokens") or 0, reported)
    cache_price = price.input if price.cache_read is None else price.cache_read
    # Uncached input may have been written to the cache: charge the dearer of the two.
    uncached_price = max(price.input, price.cache_write or 0)
    return ((reported - cached) * uncached_price + cached * cache_price
            + (usage.get("output_tokens") or 0) * price.output) / 1_000_000


USAGE_KEYS = ("requests", "input_tokens", "output_tokens", "cached_input_tokens", "reasoning_output_tokens",
              "estimated_input_tokens", "requests_missing_input_usage")


class Meter:
    """Live usage of one run: the latest totals per turn (each turn's run counts from zero),
    summed over turns. Thread-safe; fed by the runner's usage lines or the app's events."""

    def __init__(self, price):
        self.price = price
        self._turns = {}
        self._lock = threading.Lock()

    def update(self, turn, counts):
        with self._lock:
            self._turns[turn] = {key: int(counts.get(key) or 0) for key in USAGE_KEYS}

    def usage(self):
        with self._lock:
            return {key: sum(t[key] for t in self._turns.values()) for key in USAGE_KEYS}

    def cost(self):
        return cost(self.price, self.usage())


class LedgerMeter(Meter):
    """A run's live usage from both sources: `context_usage` events per run (`update`) and the
    app's usage ledger (`update_ledger`), which also counts goal checks and checkpoint summaries.
    Each count is the larger of the two (an upper bound), so neither source can hide a request."""

    def __init__(self, price):
        super().__init__(price)
        self._ledger = {key: 0 for key in USAGE_KEYS}

    def update_ledger(self, counts):
        with self._lock:
            self._ledger = {key: int(counts.get(key) or 0) for key in USAGE_KEYS}

    def usage(self):
        events = super().usage()
        with self._lock:
            ledger = dict(self._ledger)
        return {key: max(events[key], ledger[key]) for key in USAGE_KEYS}


def metrics_counts(metrics):
    """The counts of a `context_usage` event's metrics, in the runner's usage-line keys."""
    return {"requests": metrics.get("total_requests"), "input_tokens": metrics.get("provider_input_tokens"),
            "output_tokens": metrics.get("provider_output_tokens"),
            "cached_input_tokens": metrics.get("provider_cached_input_tokens"),
            "reasoning_output_tokens": metrics.get("provider_reasoning_output_tokens"),
            "estimated_input_tokens": metrics.get("estimated_input_tokens"),
            "requests_missing_input_usage": metrics.get("requests_missing_input_usage")}


class RoundSpend:
    """Spend control for a round: a run may start only when the spend so far, the running
    runs (at least their estimate each) and the new run's estimate stay within the round's
    limit. Free models are never held back."""

    def __init__(self, price, max_round_cost, max_run_cost):
        self.price, self.max_round_cost, self.max_run_cost = price, max_round_cost, max_run_cost
        self.spent = 0.0
        self.finished = {}  # size -> costs of finished runs
        self.running = {}   # key -> (meter, estimate)
        self._lock = threading.Lock()

    def estimate(self, size, material_tokens=None, price=None):
        """≈ cost of the next run of a size (at `price`, default the base price): the highest
        finished run of that size, or a prior from the size's material (the per-run limit when
        the material is unknown); never above the per-run limit (a run is stopped there)."""
        if self.price.free:
            return 0.0
        seen = self.finished.get(size)
        if not seen and material_tokens is None:
            return self.max_run_cost
        guess = max(seen) if seen else cost(price or self.price, {
            "input_tokens": material_tokens * PRIOR_INPUT_PER_MATERIAL,
            "output_tokens": material_tokens * PRIOR_OUTPUT_PER_MATERIAL})
        return min(guess, self.max_run_cost)

    def committed(self):
        with self._lock:
            return self.spent + sum(max(meter.cost(), estimate) for meter, estimate in self.running.values())

    def can_start(self, estimate):
        return self.price.free or self.committed() + estimate <= self.max_round_cost

    def start(self, key, meter, estimate):
        with self._lock:
            self.running[key] = (meter, estimate)

    def finish(self, key, size, spent):
        with self._lock:
            self.running.pop(key, None)
            self.spent += spent
            self.finished.setdefault(size, []).append(spent)

    def over_run_limit(self, meter):
        return not self.price.free and meter.cost() > self.max_run_cost


def check_model(catalog, model, budget=None, max_price=MAX_INPUT_PRICE):
    """(info, reason): the model's catalog entry when it is listed, priced and cheap enough at
    `budget` (its highest context tier), else (None, why)."""
    info = model_info(catalog, model)
    if info is None:
        return None, f"{model} is not in models.dev provider {PROVIDER}; its price is unknown"
    if isinstance(budget, int):
        price = info.price_at(budget)
    else:
        # "auto" (or no budget) can reach the model's window: price it at the dearest tier.
        price = info.tiers[-1][1] if info.tiers else info.price
    if price.input > max_price:
        return None, f"{model} costs ${price.input:.2f}/M input, above the ${max_price:.2f}/M limit"
    return info, ""


def ledger_counts(summary):
    """The app's own usage ledger (`get_usage_summary`) as meter counts: in a scratch data folder
    it holds only this run's requests, including goal checks and checkpoint summaries, which no
    `context_usage` event reports."""
    totals = ((summary or {}).get("windows") or {}).get("month") or {}
    return {"requests": totals.get("requests"), "input_tokens": totals.get("input_tokens"),
            "output_tokens": totals.get("output_tokens"),
            "cached_input_tokens": totals.get("cached_input_tokens"),
            "reasoning_output_tokens": totals.get("reasoning_tokens"),
            "requests_missing_input_usage": totals.get("unreported_requests")}


def preflight(model, base_url=None, key=None):
    """(ok, reason): whether the provider lists the model now. A model that is gone (not listed,
    or HTTP 404/410) stops a round before it starts or before its next run."""
    base = (base_url or GO_BASE_URL).rstrip("/")
    request = urllib.request.Request(f"{base}/models", headers={
        "Authorization": f"Bearer {key or os.environ.get('OPENCODE_KEY') or 'mock-key'}",
        "User-Agent": "themis-evals"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            listed = {item.get("id") for item in (json.load(response).get("data") or [])}
    except urllib.error.HTTPError as error:
        return False, f"model list: HTTP {error.code}"
    except (urllib.error.URLError, OSError, ValueError) as error:
        return False, f"model list: {type(error).__name__}"
    return (True, "") if model in listed else (False, f"{model} is not listed by the provider")


GONE = re.compile(r"\b(?:http|status|code)\D{0,6}(?:404|410)\b|\b(?:404|410) (?:not found|gone)\b"
                  r"|model[_ ]not[_ ]found|no such model", re.IGNORECASE)


def model_gone(error_text):
    """Whether a run's error says the model no longer exists (HTTP 404 or 410)."""
    return bool(GONE.search(error_text or ""))
