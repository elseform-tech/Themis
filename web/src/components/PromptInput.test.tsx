// @vitest-environment jsdom
import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { PromptInput } from "./PromptInput";

it("selects an inline skill without submitting and preserves its stable reference", () => {
  const changed = vi.fn();
  const send = vi.fn();
  function Composer() {
    const [value, setValue] = useState("/");
    return <PromptInput label="Task" value={value} onChange={next => { changed(next); setValue(next); }} skills={[{ id: "g--review--v1--rust", name: "Rust review", description: "Review changes", plugin: "review" }]} onSubmit={send} />;
  }
  render(<Composer />);
  const input = screen.getByRole("textbox", { name: "Task" });
  const range = document.createRange(); range.selectNodeContents(input); range.collapse(false);
  window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range);
  fireEvent.focus(input);
  fireEvent.keyDown(screen.getByRole("textbox", { name: "Task" }), { key: "Enter" });
  expect(changed).toHaveBeenLastCalledWith("[[skill:g--review--v1--rust]] ");
  expect(send).not.toHaveBeenCalled();
  expect(screen.getByText("Rust review")).toBeInTheDocument();
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
