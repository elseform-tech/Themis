/** A failed run's message ("Run failed: …", often a provider's whole response) as one short line:
 *  the HTTP status when there is one, a plain reason, and when to retry if the provider said. */

const REASONS: [RegExp | number, string][] = [
  [429, "Rate limited"],
  [401, "Key rejected"],
  [403, "Not allowed by the provider"],
  [404, "Model or endpoint not found"],
  [408, "Timed out"],
  [413, "Request too large"],
  [400, "Request rejected"],
  [/context|too long|maximum.*tokens/i, "Too much context for the model"],
  [/timed? ?out|deadline/i, "Timed out"],
  [/connect|network|dns|resolve|unreachable|connection/i, "Network error"],
];

function retryHint(text: string): string {
  const seconds = Number(/retry-after:\s*(\d+)/i.exec(text)?.[1]);
  if (!Number.isFinite(seconds) || seconds <= 0) return "";
  return seconds < 90 ? `, retry in ${seconds} s` : `, retry in about ${Math.round(seconds / 60)} min`;
}

export function runErrorSummary(text: string): string {
  const status = Number(/\((\d{3})\)|HTTP (\d{3})/.exec(text)?.slice(1).find(Boolean));
  const known = REASONS.find(([match]) => (typeof match === "number" ? match === status : match.test(text)))?.[1];
  const reason = known ?? (status >= 500 ? "Provider error" : firstClause(text));
  return `${status ? `${status} · ` : ""}${reason}${retryHint(text)}`;
}

/** The first clause of the message, for errors with no status or known shape. */
function firstClause(text: string): string {
  const body = text.replace(/^Run failed:\s*/, "").split(/[.:\n]/)[0].trim() || "Run failed";
  return body.length > 80 ? `${body.slice(0, 79)}…` : body;
}
