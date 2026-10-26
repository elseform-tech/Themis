import json
import shutil
import subprocess

import pytest
from ledgerly.api import handlers
from ledgerly.storage.json_store import Store


def views():
    store = Store.load("data/demo_store.json")
    return handlers.list_invoices(store)["invoices"]


def test_api_fields_follow_the_contract():
    for view in views():
        for field in ("total_cents", "paid_cents", "balance_cents"):
            assert isinstance(view[field], int), (field, view)


CHECK = """
const render = require('./web/render.js');
const invoices = JSON.parse(require('fs').readFileSync(0, 'utf8'));
const rows = render.renderRows(invoices);
const total = render.outstanding(invoices);
if (rows.includes('NaN') || total.includes('NaN') || !rows.includes(' EUR')) { console.log(rows.slice(0, 300), total); process.exit(1); }
"""


@pytest.mark.skipif(shutil.which("node") is None, reason="node is not installed")
def test_the_page_renders_the_api_without_nan():
    done = subprocess.run(["node", "-e", CHECK], input=json.dumps(views()), capture_output=True, text=True)
    assert done.returncode == 0, done.stdout + done.stderr


def test_outstanding_adds_up():
    expected = sum(view["balance_cents"] for view in views())
    assert expected == sum(view["total_cents"] - view["paid_cents"] for view in views() if view["status"] == "open")
