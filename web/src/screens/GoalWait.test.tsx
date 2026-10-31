// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GoalWait } from "./GoalWait";

beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(1_000_000); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

it("counts a retry down each second and disappears when the wait is over", () => {
  const { container } = render(<GoalWait retryAt={1_000_000 + 3_000} />);
  expect(screen.getByRole("status")).toHaveTextContent("Trying again in 3 s");
  act(() => { vi.advanceTimersByTime(1_000); });
  expect(screen.getByRole("status")).toHaveTextContent("Trying again in 2 s");
  act(() => { vi.advanceTimersByTime(2_000); });
  expect(container).toBeEmptyDOMElement();
});

it("counts longer waits in minutes and seconds", () => {
  render(<GoalWait retryAt={1_000_000 + 125_000} />);
  expect(screen.getByRole("status")).toHaveTextContent("Trying again in 2:05");
});

it("shows nothing for a wait already over, and says when a check is running", () => {
  const { container, rerender } = render(<GoalWait retryAt={1_000_000 - 1} />);
  expect(container).toBeEmptyDOMElement();
  rerender(<GoalWait checking />);
  expect(screen.getByRole("status")).toHaveTextContent("Checking…");
});
