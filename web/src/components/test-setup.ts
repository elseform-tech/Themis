import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";
import { loaderTiming } from "../screens/StartupScreen";

// The startup screen's minimum time and bar settle are for people; tests reveal the app as soon as startup ends
// (the loader's own timing test sets them back).
Object.assign(loaderTiming, { minimumMs: 0, settleMs: 0 });

// Auto-cleanup is not registered when vitest runs without `globals: true`,
// so reset the DOM explicitly between tests.
afterEach(() => {
  cleanup();
});
