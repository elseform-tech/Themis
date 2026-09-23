import { useState } from "react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { Toasts, type ToastItem } from "./Toasts";

afterEach(() => vi.useRealTimers());
it("expires each notification independently even when another arrives or is dismissed", () => {
  vi.useFakeTimers();
  function Notifications() {
    const [toasts, setToasts] = useState<ToastItem[]>([{ id: "first", message: "First error", tone: "danger" }]);
    return <><button onClick={() => setToasts(items => [...items, { id: "second", message: "Second notice", tone: "info" }])}>Add</button><Toasts toasts={toasts} onDismiss={id => setToasts(items => items.filter(item => item.id !== id))} /></>;
  }
  render(<Notifications />);
  act(() => vi.advanceTimersByTime(4000));
  fireEvent.click(screen.getByText("Add"));
  act(() => vi.advanceTimersByTime(1999));
  expect(screen.getByText("First error")).toBeInTheDocument();
  act(() => vi.advanceTimersByTime(1));
  expect(screen.queryByText("First error")).toBeNull();
  expect(screen.getByText("Second notice")).toBeInTheDocument();
  act(() => vi.advanceTimersByTime(4000));
  expect(screen.queryByText("Second notice")).toBeNull();
  fireEvent.click(screen.getByText("Add"));
  fireEvent.click(screen.getByRole("button", { name: "Dismiss notification" }));
  expect(screen.queryByText("Second notice")).toBeNull();
});
