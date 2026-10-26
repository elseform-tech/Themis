"""Products sold by the shop."""

from dataclasses import dataclass


@dataclass
class Product:
    sku: str
    name: str
    unit_cents: int
    taxable: bool = True

    def to_dict(self):
        return {"sku": self.sku, "name": self.name, "unit_cents": self.unit_cents, "taxable": self.taxable}

    @classmethod
    def from_dict(cls, data):
        return cls(data["sku"], data["name"], int(data["unit_cents"]), bool(data.get("taxable", True)))
