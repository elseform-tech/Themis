// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/tauri";
import { initialState } from "../state/reducer";
import { SecretsSettings } from "./SecretsSettings";

vi.mock("../lib/tauri", () => ({ listKeys: vi.fn(), setKey: vi.fn(async () => undefined), removeKey: vi.fn(async () => undefined) }));
vi.mock("../state/store", async original => ({ ...await original<typeof import("../state/store")>(), useApp: () => ({ state: { ...initialState, activeProjectRoot: "/project" }, dispatch: vi.fn() }) }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const KEYS = [
  { name: "GITHUB_TOKEN", saved: true, environment: false, used_by: ["personal/github"] },
  { name: "NOTES_KEY", saved: false, environment: false, used_by: ["remote/notes"] },
];

describe("Settings › Secrets", () => {
  it("lists secrets with their users and state, never a value", async () => {
    vi.mocked(bridge.listKeys).mockResolvedValue(KEYS);
    await act(async () => { render(<SecretsSettings />); });
    expect(bridge.listKeys).toHaveBeenCalledWith("/project");
    expect(screen.getByText("personal/github")).toBeVisible();
    expect(screen.getByText("Saved")).toBeVisible();
    expect(screen.getByText("Missing")).toBeVisible();
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(1);
  });

  it("opens on the secret a server asked for and saves it", async () => {
    vi.mocked(bridge.listKeys).mockResolvedValue(KEYS);
    await act(async () => { render(<SecretsSettings focusKey="NOTES_KEY" />); });
    const input = screen.getByLabelText("Value of NOTES_KEY");
    expect(input).toHaveAttribute("type", "password");
    fireEvent.change(input, { target: { value: " abc " } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save" })); });
    expect(bridge.setKey).toHaveBeenCalledWith("NOTES_KEY", "abc");
    expect(bridge.listKeys).toHaveBeenCalledTimes(2);
  });

  it("adds a new named secret and refuses a bad name", async () => {
    vi.mocked(bridge.listKeys).mockResolvedValue([]);
    await act(async () => { render(<SecretsSettings />); });
    fireEvent.change(screen.getByLabelText("New secret name"), { target: { value: "bad-name" } });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(screen.getByRole("alert")).toHaveTextContent("letters, digits and _");
    fireEvent.change(screen.getByLabelText("New secret name"), { target: { value: "API_KEY" } });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(screen.getByLabelText("Value of API_KEY")).toBeVisible();
  });

  it("removes a saved secret", async () => {
    vi.mocked(bridge.listKeys).mockResolvedValue(KEYS);
    await act(async () => { render(<SecretsSettings />); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Remove" })); });
    expect(bridge.removeKey).toHaveBeenCalledWith("GITHUB_TOKEN");
  });
});
