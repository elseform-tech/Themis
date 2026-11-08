import shutil
import subprocess

import pytest

# A stand-in page: the real app.js wired to stub elements, then the Show menu changed to "paid".
CHECK = """
const elements = {};
const element = id => elements[id] || (elements[id] = {
  id, value: '', hidden: false, innerHTML: '', textContent: '', listeners: {},
  addEventListener(type, listener) { this.listeners[type] = listener; },
});
global.document = { getElementById: element };
const asked = [];
global.fetchInvoices = async status => { asked.push(status); return []; };
global.renderRows = () => '';
global.outstanding = () => '';
require('./web/app.js');
(async () => {
  await new Promise(done => setTimeout(done, 20));
  element('status-filter').value = 'paid';
  await element('status-filter').listeners.change();
  console.log(JSON.stringify(asked));
  process.exit(asked[asked.length - 1] === 'paid' ? 0 : 1);
})();
"""


@pytest.mark.skipif(shutil.which("node") is None, reason="node is not installed")
def test_changing_the_show_menu_asks_for_that_status():
    done = subprocess.run(["node", "-e", CHECK], capture_output=True, text=True)
    assert done.returncode == 0, done.stdout + done.stderr


def test_the_api_filters_by_status():
    from ledgerly.api import handlers
    from ledgerly.storage.json_store import Store

    store = Store.load("data/demo_store.json")
    paid = handlers.list_invoices(store, {"status": "paid"})["invoices"]
    assert paid and all(view["status"] == "paid" for view in paid)
