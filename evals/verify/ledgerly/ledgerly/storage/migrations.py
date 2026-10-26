"""Schema migrations for the JSON store. Each step upgrades one version; `migrate` runs every
step a document needs. Never edit a released step: add a new one (docs/decisions/0004-storage.md).
"""

# >>> plant:feature-discount-codes
CURRENT_SCHEMA = 2
# === plant:feature-discount-codes
CURRENT_SCHEMA = 3
# <<< plant:feature-discount-codes


def _v1_to_v2(data):
    """0.6 stored line prices as "price" in euros (text); 0.7 stores "unit_cents"."""
    from ledgerly.core import money

    for invoice in data.get("invoices", []):
        for line in invoice.get("lines", []):
            if "price" in line:
                line["unit_cents"] = money.to_cents(line.pop("price"))
    return data
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes


def _v2_to_v3(data):
    """0.9 adds discount codes: older invoices have none."""
    for invoice in data.get("invoices", []):
        invoice.setdefault("discount_code", None)
    return data
# <<< plant:feature-discount-codes


STEPS = {
    1: _v1_to_v2,
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
    2: _v2_to_v3,
# <<< plant:feature-discount-codes
}


def migrate(data):
    """`data` upgraded in place to CURRENT_SCHEMA; a newer schema than this code knows is an error."""
    version = int(data.get("schema", 1))
    if version > CURRENT_SCHEMA:
        raise ValueError(f"store schema {version} is newer than this Ledgerly ({CURRENT_SCHEMA})")
    while version < CURRENT_SCHEMA:
        data = STEPS[version](data)
        version += 1
    data["schema"] = CURRENT_SCHEMA
    return data
