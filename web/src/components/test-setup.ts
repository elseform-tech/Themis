import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// Auto-cleanup is not registered when vitest runs without `globals: true`,
// so reset the DOM explicitly between tests.
afterEach(() => {
  cleanup();
});
