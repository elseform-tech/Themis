// Turns API invoices (see docs/ARCHITECTURE.md, "API") into table rows. Loaded by the page and
// by node (module.exports) for checks.

function formatCents(cents) {
  const sign = cents < 0 ? "-" : "";
  const value = Math.abs(cents);
  return `${sign}${Math.floor(value / 100)}.${String(value % 100).padStart(2, "0")} EUR`;
}

function escapeHtml(text) {
  return String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
}

function renderRows(invoices) {
  return invoices
    .map(
      (invoice) =>
        `<tr class="status-${escapeHtml(invoice.status)}"><td>${escapeHtml(invoice.id)}</td>` +
        `<td>${escapeHtml(invoice.customer)}</td><td>${escapeHtml(invoice.issued)}</td>` +
        `<td>${escapeHtml(invoice.status)}</td><td class="num">${formatCents(invoice.total_cents)}</td>` +
        `<td class="num">${formatCents(invoice.balance_cents)}</td></tr>`,
    )
    .join("");
}

function outstanding(invoices) {
  return formatCents(invoices.reduce((sum, invoice) => sum + invoice.balance_cents, 0));
}

if (typeof module !== "undefined") {
  module.exports = { formatCents, renderRows, outstanding };
}
