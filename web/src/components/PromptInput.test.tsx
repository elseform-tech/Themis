// @vitest-environment jsdom
import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { PromptInput, PromptText } from "./PromptInput";
import { pluginToken, promptParts } from "../lib/prompt";
import { readFileSync } from "node:fs";

const plugins = [
  { id: "g--drive", name: "Google Drive", description: "Drive tools", origin: "Global · personal", icon: "https://example.com/drive.svg", enabled: true },
  { id: "l--disabled", name: "Disabled", description: "Hidden", origin: "Project", enabled: false },
];

it.each(["global","local"])("shows a readable %s discovery origin while retaining canonical matching and selection",scope=>{
  const changed=vi.fn();
  const id=`${scope==="global"?"g":"l"}--discovered-0123456789abcdef--review`;
  const description="Review this project.\n\nA longer description with more instructions.";
  render(<PromptInput label="Discovered" value={`/${id}`} onChange={changed} skills={[{id,name:"Readable review",description,plugin:"discovered-0123456789abcdef",scope}]} />);
  const input=screen.getByRole("textbox",{name:"Discovered"});
  const range=document.createRange();range.selectNodeContents(input);range.collapse(false);
  window.getSelection()!.removeAllRanges();window.getSelection()!.addRange(range);
  fireEvent.focus(input);
  const option=screen.getByRole("option");
  expect(option).toHaveTextContent(`${scope==="global"?"User":"Project"} · Discovered skills`);
  expect(option).not.toHaveTextContent("discovered-0123456789abcdef");
  expect(option.querySelector("small")).toHaveAttribute("title",description);
  fireEvent.keyDown(input,{key:"Enter"});
  expect(changed).toHaveBeenLastCalledWith(`[[skill:${id}]] `);
});

it.each(["/","@"])("scrolls the selected %s option into view for ArrowDown, ArrowUp and wraparound",trigger=>{
  const previous=HTMLElement.prototype.scrollIntoView;
  const targets:HTMLElement[]=[];
  const scroll=vi.fn(function(this:HTMLElement){targets.push(this);});
  HTMLElement.prototype.scrollIntoView=scroll;
  try {
    const skills=Array.from({length:14},(_,index)=>({id:`skill-${index}`,name:`Skill ${index}`,description:"A longer description",plugin:"Tools"}));
    const choices=Array.from({length:14},(_,index)=>({id:`g--plugin-${index}`,name:`Plugin ${index}`,description:"A longer description",origin:"Global",enabled:true}));
    render(<PromptInput label="Long suggestions" value={trigger} onChange={vi.fn()} skills={skills} plugins={choices} />);
    const input=screen.getByRole("textbox",{name:"Long suggestions"});
    const range=document.createRange();range.selectNodeContents(input);range.collapse(false);
    window.getSelection()!.removeAllRanges();window.getSelection()!.addRange(range);
    fireEvent.focus(input);
    const options=screen.getAllByRole("option");
    expect(options).toHaveLength(12);
    for(let index=1;index<options.length;index++) {
      fireEvent.keyDown(input,{key:"ArrowDown"});
      expect(options[index]).toHaveAttribute("aria-selected","true");
      expect(targets[targets.length-1]).toBe(options[index]);
    }
    fireEvent.keyDown(input,{key:"ArrowUp"});
    expect(targets[targets.length-1]).toBe(options[10]);
    fireEvent.keyDown(input,{key:"ArrowDown"});
    fireEvent.keyDown(input,{key:"ArrowDown"});
    expect(targets[targets.length-1]).toBe(options[0]);
    fireEvent.keyDown(input,{key:"ArrowUp"});
    expect(targets[targets.length-1]).toBe(options[11]);
    expect(scroll).toHaveBeenLastCalledWith({block:"nearest",inline:"nearest"});
  } finally {HTMLElement.prototype.scrollIntoView=previous;}
});

const builtinSkills=[
  ["create-skill","Create skill"],
  ["manage-plugins","Manage plugins"],
  ["manage-skills","Manage skills"],
  ["manage-mcp","Manage MCP"],
  ["manage-hooks","Manage hooks"],
  ["manage-automations","Manage automations"],
].map(([id,name])=>({id,name,description:"Manage capabilities",plugin:"Themis"}));

it.each(builtinSkills.flatMap(skill=>["Enter","Tab"].map(key=>({skill,key}))))("selects /$skill.id with $key using its canonical command",({skill,key})=>{
  const changed=vi.fn(),send=vi.fn();
  render(<PromptInput label="Management" value={`Please /${skill.id}`} onChange={changed} skills={builtinSkills} onSubmit={send} />);
  const input=screen.getByRole("textbox",{name:"Management"});
  const range=document.createRange();range.selectNodeContents(input);range.collapse(false);
  window.getSelection()!.removeAllRanges();window.getSelection()!.addRange(range);
  fireEvent.focus(input);
  expect(screen.getByRole("option",{name:new RegExp(skill.name)})).toBeInTheDocument();
  fireEvent.keyDown(input,{key});
  expect(changed).toHaveBeenLastCalledWith(`Please [[skill:${skill.id}]] `);
  expect(send).not.toHaveBeenCalled();
});

it("matches the hyphenated management command prefix seen in the native composer",()=>{
  render(<PromptInput label="Prefix" value="/manage-sk" onChange={vi.fn()} skills={builtinSkills} />);
  const input=screen.getByRole("textbox",{name:"Prefix"});
  const range=document.createRange();range.selectNodeContents(input);range.collapse(false);
  window.getSelection()!.removeAllRanges();window.getSelection()!.addRange(range);
  fireEvent.focus(input);
  expect(screen.getByRole("option",{name:/Manage skills/})).toBeInTheDocument();
  expect(screen.queryByText("No matching skills")).toBeNull();
});

it("keeps reference icons and labels together despite Tailwind's block image reset",()=>{
  const style=document.createElement("style");
  style.textContent=readFileSync("node_modules/tailwindcss/preflight.css","utf8")+readFileSync("src/components/PromptInput.css","utf8");
  document.head.append(style);
  try {
    const {container}=render(<PromptText value="Please [[plugin:g--drive]] and [[skill:review]] for me" plugins={plugins} skills={[{id:"review",name:"Review",description:"Code",plugin:"review"}]} />);
    const references=container.querySelectorAll(".themis-skill-chip");
    for (const reference of references) {
      expect(getComputedStyle(reference).display).toBe("inline-flex");
      expect(getComputedStyle(reference).padding).toBe("0px");
      expect(getComputedStyle(reference).verticalAlign).toBe("baseline");
      expect(getComputedStyle(reference.querySelector("img,svg")!).display).toBe("inline-block");
    }
  } finally {style.remove();}
});

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
  expect(input.querySelector('[data-plugin="g--drive"] .themis-shimmer')).toHaveTextContent("Google Drive");
  expect(input.querySelector('[data-plugin="g--drive"] img')).not.toHaveClass("themis-shimmer");
});

it("parses persisted mixed plugin and skill references and renders their separate chips", () => {
  const value = `${pluginToken("g--drive")} use [[skill:g--review--rust]]`;
  expect(promptParts(value)).toEqual([{ plugin: "g--drive" }, { text: " use " }, { skill: "g--review--rust" }]);
  render(<PromptText value={value} skills={[{ id: "g--review--rust", name: "Review", description: "Review code", plugin: "review" }]} plugins={plugins} />);
  expect(screen.getByText("Google Drive")).toBeInTheDocument();
  expect(screen.getByText("Review")).toBeInTheDocument();
  expect(screen.getByText("Google Drive")).toHaveClass("themis-shimmer");
  expect(screen.getByText("Review")).toHaveClass("themis-shimmer");
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
  expect(input.querySelector('[data-skill="g--review--v1--rust"] .themis-shimmer')).toHaveTextContent("Rust review");
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

it("serializes mixed inline references when one is deleted and restores its marker on undo",()=>{
  const changed=vi.fn();
  function Composer() {
    const [value,setValue]=useState("Please [[plugin:g--drive]] and [[skill:review]] today");
    return <PromptInput label="Mixed" value={value} onChange={next=>{changed(next);setValue(next);}} plugins={plugins} skills={[{id:"review",name:"Review",description:"Code",plugin:"review"}]} />;
  }
  render(<Composer />);
  const input=screen.getByRole("textbox",{name:"Mixed"});
  input.querySelector('[data-plugin="g--drive"]')!.remove();
  fireEvent.input(input);
  expect(changed).toHaveBeenLastCalledWith("Please  and [[skill:review]] today");
  fireEvent.keyDown(input,{key:"z",ctrlKey:true});
  expect(changed).toHaveBeenLastCalledWith("Please [[plugin:g--drive]] and [[skill:review]] today");
  expect(input.querySelector('[data-plugin="g--drive"] .themis-shimmer')).toHaveTextContent("Google Drive");
  expect(input.querySelector('[data-skill="review"] .themis-shimmer')).toHaveTextContent("Review");
});

it("preserves the text caret when metadata refresh rebuilds mixed shimmer references",()=>{
  const value="Please [[plugin:g--drive]] and [[skill:review]] today";
  const props={label:"Refreshing",value,onChange:vi.fn(),plugins,skills:[{id:"review",name:"Review",description:"Code",plugin:"review"}]};
  const view=render(<PromptInput {...props} />);
  const input=screen.getByRole("textbox",{name:"Refreshing"});
  const suffix=input.lastChild!;
  const range=document.createRange();range.setStart(suffix,3);range.collapse(true);
  window.getSelection()!.removeAllRanges();window.getSelection()!.addRange(range);
  view.rerender(<PromptInput {...props} skills={[{...props.skills[0],name:"Updated review"}]} />);
  expect(window.getSelection()!.anchorNode?.textContent).toBe(" today");
  expect(window.getSelection()!.anchorOffset).toBe(3);
  expect(input.querySelector('[data-skill="review"] .themis-shimmer')).toHaveTextContent("Updated review");
  expect(props.onChange).not.toHaveBeenCalled();
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

it("handles WebKit retaining a stale element caret after the composer is cleared", () => {
  const view=render(<PromptInput label="Cleared" value="" onChange={vi.fn()} skills={builtinSkills} />);
  const input=screen.getByRole("textbox",{name:"Cleared"});
  const range=document.createRange();range.selectNodeContents(input);range.collapse(false);
  const selection=vi.spyOn(window,"getSelection").mockReturnValue({rangeCount:1,anchorNode:input,anchorOffset:7,getRangeAt:()=>range,removeAllRanges:vi.fn(),addRange:vi.fn()} as unknown as Selection);
  try {
    expect(()=>view.rerender(<PromptInput label="Cleared" value="" onChange={vi.fn()} skills={[]} />)).not.toThrow();
    expect(input).toHaveTextContent("");
    expect(screen.queryByRole("listbox")).toBeNull();
  } finally {selection.mockRestore();}
});
