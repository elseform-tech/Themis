"""Customers."""

from dataclasses import dataclass


@dataclass
class Customer:
    id: str
    name: str
    country: str = "DE"
    terms_days: int = 14
    email: str = ""

    def to_dict(self):
        return {"id": self.id, "name": self.name, "country": self.country,
                "terms_days": self.terms_days, "email": self.email}

    @classmethod
    def from_dict(cls, data):
        return cls(data["id"], data["name"], data.get("country", "DE"),
                   int(data.get("terms_days", 14)), data.get("email", ""))
