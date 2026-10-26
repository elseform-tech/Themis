// Talks to the JSON API served by `python3 -m ledgerly.api.routes`.

async function fetchInvoices(status) {
  const query = status ? `?status=${encodeURIComponent(status)}` : "";
  const response = await fetch(`/api/invoices${query}`);
  if (!response.ok) {
    throw new Error(`API answered ${response.status}`);
  }
  const body = await response.json();
  return body.invoices;
}
