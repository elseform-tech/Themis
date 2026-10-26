"""Stock levels per product."""

from dataclasses import dataclass


@dataclass
class StockLevel:
    sku: str
    on_hand: int = 0
    reserved: int = 0

    @property
    def available(self):
        return self.on_hand - self.reserved

    def to_dict(self):
        return {"sku": self.sku, "on_hand": self.on_hand, "reserved": self.reserved}

    @classmethod
    def from_dict(cls, data):
        return cls(data["sku"], int(data.get("on_hand", 0)), int(data.get("reserved", 0)))
