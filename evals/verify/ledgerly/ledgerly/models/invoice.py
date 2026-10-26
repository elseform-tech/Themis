"""Invoices: lines of products at a unit price, for one customer."""

from dataclasses import dataclass, field
from datetime import date, datetime

from ledgerly.core import dates

STATUSES = ("open", "paid", "void")


@dataclass
class Line:
    sku: str
    quantity: int
    unit_cents: int

    def to_dict(self):
        return {"sku": self.sku, "quantity": self.quantity, "unit_cents": self.unit_cents}

    @classmethod
    def from_dict(cls, data):
        return cls(data["sku"], int(data["quantity"]), int(data["unit_cents"]))


@dataclass
class Invoice:
    id: str
    customer_id: str
    issued: date
    lines: list = field(default_factory=list)
    status: str = "open"
    paid_cents: int = 0
    created_at: datetime = None
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
    discount_code: str = None
# <<< plant:feature-discount-codes

    def to_dict(self):
        data = {"id": self.id, "customer_id": self.customer_id, "issued": self.issued.isoformat(),
                "lines": [line.to_dict() for line in self.lines], "status": self.status,
                "paid_cents": self.paid_cents,
                "created_at": self.created_at.isoformat() if self.created_at else None}
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
        data["discount_code"] = self.discount_code
# <<< plant:feature-discount-codes
        return data

    @classmethod
    def from_dict(cls, data):
        invoice = cls(data["id"], data["customer_id"], date.fromisoformat(data["issued"]),
                      [Line.from_dict(line) for line in data.get("lines", [])],
                      data.get("status", "open"), int(data.get("paid_cents", 0)),
                      dates.parse_moment(data["created_at"]) if data.get("created_at") else None)
# >>> plant:feature-discount-codes
# === plant:feature-discount-codes
        invoice.discount_code = data.get("discount_code")
# <<< plant:feature-discount-codes
        return invoice
