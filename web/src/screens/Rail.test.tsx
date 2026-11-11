// @vitest-environment jsdom
import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS, initialState } from "../state/reducer";
import type { ReviewItem } from "../lib/types";
import { Rail } from "./Rail";

const dispatch = vi.fn();
let reviews: ReviewItem[] = [];
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: { ...initialState, settings: DEFAULT_SETTINGS, reviews }, dispatch }),
}));
vi.mock("../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../lib/tauri")>(),
  pluginAction: vi.fn(async () => []),
}));

const review = (id: string, status: ReviewItem["status"]) => ({ id, status } as ReviewItem);

beforeEach(() => { reviews = []; dispatch.mockClear(); });

describe("Rail", () => {
  it("renders today's rail in order", () => {
    render(<Rail onHelp={() => undefined} />);
    const names = screen.getAllByRole("button").map(button => button.getAttribute("aria-label"));
    expect(names).toEqual(["Chats", "Integrations", "Automations", "Review queue", "Settings", "Help", "Search commands"]);
    expect(screen.getByRole("navigation", { name: "Utilities" })).toBeTruthy();
  });

  it("chats is first and selected on the thread view", () => {
    render(<Rail onHelp={() => undefined} />);
    const first = screen.getAllByRole("button")[0];
    expect(first.getAttribute("aria-label")).toBe("Chats");
    expect(first.getAttribute("aria-current")).toBe("page");
  });

  it("extra renders after chats", () => {
    render(<Rail onHelp={() => undefined} extra={<button type="button" aria-label="Visions" />} />);
    const names = screen.getAllByRole("button").map(button => button.getAttribute("aria-label"));
    expect(names.slice(0, 3)).toEqual(["Chats", "Visions", "Integrations"]);
  });

  it("has no sidebar toggle in the rail", () => {
    render(<Rail onHelp={() => undefined} />);
    expect(within(screen.getByRole("navigation", { name: "Utilities" })).queryByRole("button", { name: /sidebar/i })).toBeNull();
  });

  it("review badge counts pending reviews", () => {
    reviews = [review("a", "pending"), review("b", "pending"), review("c", "done" as ReviewItem["status"])];
    render(<Rail onHelp={() => undefined} />);
    expect(screen.getByLabelText("Review queue 2")).toBeTruthy();
  });

  it("calls onHelp from the Help button", () => {
    const onHelp = vi.fn();
    render(<Rail onHelp={onHelp} />);
    screen.getByLabelText("Help").click();
    expect(onHelp).toHaveBeenCalled();
  });
});
