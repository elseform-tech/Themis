import { describe, expect, it } from "vitest";
import { runErrorSummary } from "./runError";

describe("runErrorSummary", () => {
  it("gives the status, a plain reason and when to retry", () => {
    const text = 'Run failed: LLM request failed: Rate Limit Error (429): OpenCode Go request failed with HTTP 429. Response: {"error":{"type":"rate_limit_error","message":"Rate limit exceeded. Please try again later."}}. Retry-After: 4194s';
    expect(runErrorSummary(text)).toBe("429 · Rate limited, retry in about 70 min");
  });

  it("names common provider failures", () => {
    expect(runErrorSummary("Run failed: request failed with HTTP 401. Response: {}")).toBe("401 · Key rejected");
    expect(runErrorSummary("Run failed: Server Error (503): upstream")).toBe("503 · Provider error");
    expect(runErrorSummary("Run failed: Rate Limit Error (429). Retry-After: 30s")).toBe("429 · Rate limited, retry in 30 s");
  });

  it("reads errors with no status", () => {
    expect(runErrorSummary("Run failed: error sending request: connection refused")).toBe("Network error");
    expect(runErrorSummary("Run failed: the request timed out after 300 s")).toBe("Timed out");
    expect(runErrorSummary("Run failed: tool loop exceeded its budget")).toBe("tool loop exceeded its budget");
  });
});
