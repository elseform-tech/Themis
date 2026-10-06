// @vitest-environment jsdom
import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { PromptInput, PromptText } from "./PromptInput";
import { pluginToken, promptParts } from "../lib/prompt";

const plugins = [
  { id: "g--drive", name: "Google Drive", description: "Drive tools", origin: "Global · personal", icon: "https://example.com/drive.svg", enabled: true },
  { id: "l--disabled", name: "Disabled", description: "Hidden", origin: "Project", enabled: false },
];

it.each(["Enter", "Tab"])("selects an @ plugin with %s without submitting and preserves its scoped marker", key => {
  const changed = vi.fn(), send = vi.fn();
  function Composer() {
    const [value, setValue] = useState("@");
    return <PromptInput label="Plugin message" value={value} onChange={next => { changed(next); setValue(next); }} skills={[]} plugins={plugins} onSubmit={send} />;
  }
  render(<Composer />);
  const input = screen.getByRole("textbox", { name: "Plugin message" });
  const range = document.createRange(); range.selectNodeContents(input); range.collapse(false);
  window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range);
  fireEvent.focus(input);
  expect(screen.getByRole("listbox", { name: "Plugins" })).toBeInTheDocument();
  expect(screen.queryByRole("option", { name: /Disabled/ })).toBeNull();
  expect(screen.getByRole("option", { name: /Google Drive/ }).querySelector("img")?.getAttribute("src")).toBe(plugins[0].icon);
  fireEvent.keyDown(input, { key });
  expect(changed).toHaveBeenLastCalledWith("[[plugin:g--drive]] ");
  expect(send).not.toHaveBeenCalled();
  expect(input.querySelector('[data-plugin="g--drive"]')).toHaveTextContent("Google Drive");
});

it("parses persisted mixed plugin and skill references and renders their separate chips", () => {
  const value = `${pluginToken("g--drive")} use [[skill:g--review--rust]]`;
  expect(promptParts(value)).toEqual([{ plugin: "g--drive" }, { text: " use " }, { skill: "g--review--rust" }]);
  render(<PromptText value={value} skills={[{ id: "g--review--rust", name: "Review", description: "Review code", plugin: "review" }]} plugins={plugins} />);
  expect(screen.getByText("Google Drive")).toBeInTheDocument();
  expect(screen.getByText("Review")).toBeInTheDocument();
});

it.each(["Enter", "Tab"])("selects an inline / skill with %s without submitting and preserves its stable reference", key => {
  const changed = vi.fn();
  const send = vi.fn();
  function Composer() {
    const [value, setValue] = useState("/");
    return <PromptInput label="Task" value={value} onChange={next => { changed(next); setValue(next); }} skills={[{ id: "g--review--v1--rust", name: "Rust review", description: "Review changes", plugin: "review" }]} plugins={plugins} onSubmit={send} />;
  }
  render(<Composer />);
  const input = screen.getByRole("textbox", { name: "Task" });
  const range = document.createRange(); range.selectNodeContents(input); range.collapse(false);
  window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range);
  fireEvent.focus(input);
  expect(screen.getByRole("listbox", { name: "Skills" })).toBeInTheDocument();
  expect(screen.queryByRole("option", { name: /Google Drive/ })).toBeNull();
  fireEvent.keyDown(screen.getByRole("textbox", { name: "Task" }), { key });
  expect(changed).toHaveBeenLastCalledWith("[[skill:g--review--v1--rust]] ");
  expect(send).not.toHaveBeenCalled();
  expect(screen.getByText("Rust review")).toBeInTheDocument();
});

it.each(["mail@example.com", "`@drive", "```\n@drive"])("does not offer plugin mentions inside %s", value => {
  render(<PromptInput label="Literal" value={value} onChange={vi.fn()} skills={[]} plugins={plugins} />);
  const input=screen.getByRole("textbox",{name:"Literal"});
  const range=document.createRange();range.selectNodeContents(input);range.collapse(false);
  window.getSelection()!.removeAllRanges();window.getSelection()!.addRange(range);
  fireEvent.focus(input);
  expect(screen.queryByRole("listbox")).toBeNull();
});

it("recalls prompts and restores the unfinished draft", () => {
  const changed = vi.fn();
  render(<PromptInput label="Message" value="draft" onChange={changed} skills={[]} history={["older", "latest"]} />);
  const editor = screen.getByRole("textbox", { name: "Message" });
  fireEvent.keyDown(editor, { key: "ArrowUp" });
  expect(changed).toHaveBeenLastCalledWith("latest");
  fireEvent.keyDown(editor, { key: "ArrowUp" });
  expect(changed).toHaveBeenLastCalledWith("older");
  fireEvent.keyDown(editor, { key: "ArrowDown" });
  fireEvent.keyDown(editor, { key: "ArrowDown" });
  expect(changed).toHaveBeenLastCalledWith("draft");
});

it.each(["/", "/Users"])("preserves Shift+Enter and Tab navigation for %s", value => {
  const changed = vi.fn(), send = vi.fn();
  render(<PromptInput label="Task" value={value} onChange={changed} skills={[{ id: "review", name: "Review", description: "Review changes", plugin: "review" }]} onSubmit={send} />);
  const input = screen.getByRole("textbox", { name: "Task" });
  const range = document.createRange(); range.selectNodeContents(input); range.collapse(false);
  window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range);
  fireEvent.focus(input);
  expect(fireEvent.keyDown(input, { key: "Enter", shiftKey: true })).toBe(true);
  expect(fireEvent.keyDown(input, { key: "Tab", shiftKey: true })).toBe(true);
  expect(changed).not.toHaveBeenCalled();
  expect(send).not.toHaveBeenCalled();
  if (value === "/Users") {
    expect(fireEvent.keyDown(input, { key: "Tab" })).toBe(true);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(send).toHaveBeenCalledTimes(1);
  }
});
