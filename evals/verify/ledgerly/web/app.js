// Wires the page: load invoices, render them, refilter on change.

async function refresh() {
  const filter = document.getElementById("status-filter").value;
  const error = document.getElementById("error");
  try {
    const invoices = await fetchInvoices(filter);
    document.getElementById("invoice-rows").innerHTML = renderRows(invoices);
    document.getElementById("outstanding").textContent = outstanding(invoices);
    error.hidden = true;
  } catch (failure) {
    error.textContent = `Could not load invoices: ${failure.message}`;
    error.hidden = false;
  }
}

document.getElementById("status-filter").addEventListener("change", refresh);
refresh();
