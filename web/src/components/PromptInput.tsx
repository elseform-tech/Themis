import { useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { promptParts, skillToken, type PromptSkill } from "../lib/prompt";
import "./PromptInput.css";

interface Props { value:string; onChange:(value:string)=>void; skills:PromptSkill[]; label:string; id?:string; disabled?:boolean; placeholder?:string; onSubmit?:()=>void; history?:string[] }
function serialize(node:Node):string {
  if (node.nodeType===Node.TEXT_NODE) return node.textContent ?? "";
  if (node instanceof HTMLElement && node.dataset.skill) return skillToken(node.dataset.skill);
  if (node instanceof HTMLBRElement) return "\n";
  const text=Array.from(node.childNodes).map(serialize).join("");
  return node instanceof HTMLElement && ["DIV","P"].includes(node.tagName) && node.previousSibling ? "\n"+text : text;
}
function caret(element:HTMLElement):number {
  const selection=window.getSelection();
  if (!selection?.rangeCount || !element.contains(selection.anchorNode)) return 0;
  const range=selection.getRangeAt(0).cloneRange();range.selectNodeContents(element);range.setEnd(selection.anchorNode!,selection.anchorOffset);
  return serialize(range.cloneContents()).length;
}
export function SkillIcon() { return <svg width="14" height="14" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><path d="m8 2 1.6 4.4L14 8l-4.4 1.6L8 14l-1.6-4.4L2 8l4.4-1.6L8 2Zm7 10 .8 2.2L18 15l-2.2.8L15 18l-.8-2.2L12 15l2.2-.8L15 12Z"/></svg>; }
export function PromptText({value,skills}:{value:string;skills:PromptSkill[]}) {
  return <>{promptParts(value).map((part,index)=>"text" in part ? part.text : <span key={index} className="themis-skill-chip" title={skills.find(s=>s.id===part.skill)?.plugin ?? "Unavailable skill"}><SkillIcon />{skills.find(s=>s.id===part.skill)?.name ?? (part.skill==="create-skill"?"Create skill":"Unavailable skill")}</span>)}</>;
}
export function PromptInput({value,onChange,skills,label,id,disabled,placeholder,onSubmit,history=[]}:Props) {
  const editor=useRef<HTMLDivElement>(null),pendingCaret=useRef<number|null>(null),recall=useRef({index:-1,draft:""}),undo=useRef<string[]>([]),redo=useRef<string[]>([]);
  const [slash,setSlash]=useState<{start:number;end:number;query:string}|null>(null);
  const [active,setActive]=useState(0);
  const labels = skills.map(s=>`${s.id}:${s.name}`).join("|");
  const previousLabels = useRef(labels);
  const choices=skills.filter(s=>!s.retired).filter(s=>!slash?.query || slash.query==="skill" || `${s.name} ${s.plugin} ${s.id==="create-skill"?"create-skill":""}`.toLowerCase().includes(slash.query.toLowerCase())).slice(0,12);
  const listId=(id ?? "prompt")+"-skills";
  function update(next:string) {undo.current.push(value);redo.current=[];onChange(next);}
  function detectSlash() {
    const element=editor.current;if (!element)return;
    const end=caret(element),prefix=serialize(element).slice(0,end);
    const match=prefix.match(/(?:^|\s)\/([\w:.-]*)$/);
    if (!match || (prefix.match(/```/g)?.length ?? 0)%2 || (prefix.split("\n").slice(-1)[0]?.match(/`/g)?.length ?? 0)%2) {setSlash(null);return;}
    setSlash({start:end-match[1].length-1,end,query:match[1]});setActive(0);
  }
  useLayoutEffect(()=>{
    const element=editor.current;if (!element)return;
    const labelsChanged = previousLabels.current !== labels;
    previousLabels.current = labels;
    if (labelsChanged && element.contains(window.getSelection()?.anchorNode ?? null)) pendingCaret.current = caret(element);
    if (serialize(element)!==value || element.childNodes.length===0 || labelsChanged) {
      element.replaceChildren();
      for (const part of promptParts(value)) {
        if ("text" in part) {element.append(document.createTextNode(part.text));continue;}
        const skill=skills.find(s=>s.id===part.skill),chip=document.createElement("span");
        chip.className="themis-skill-chip";chip.contentEditable="false";chip.dataset.skill=part.skill;
        chip.title=skill ? `${skill.plugin} · ${skill.description}` : "Unavailable skill; replace before running";
        const icon=document.createElement("span");icon.textContent=skill?"✧":"⚠";icon.setAttribute("aria-hidden","true");
        chip.append(icon,document.createTextNode(skill?.name ?? (part.skill==="create-skill"?"Create skill":"Unavailable skill")));
        element.append(chip);
      }
      if (!element.lastChild || (element.lastChild instanceof HTMLElement && element.lastChild.dataset.skill)) element.append(document.createTextNode(""));
    }
    if (pendingCaret.current!==null) {
      let offset=pendingCaret.current;const range=document.createRange();range.selectNodeContents(element);range.collapse(false);
      for (const node of Array.from(element.childNodes)) {
        const length=serialize(node).length;
        if (offset<=length) {if(node.nodeType===Node.TEXT_NODE)range.setStart(node,offset);else range.setStartAfter(node);range.collapse(true);break;}offset-=length;
      }
      const selection=window.getSelection();selection?.removeAllRanges();selection?.addRange(range);pendingCaret.current=null;
    }
  },[value,skills,labels]);
  function select(skill:PromptSkill) {
    if (!slash)return;
    const token=skillToken(skill.id)+" ";pendingCaret.current=slash.start+token.length;
    update(value.slice(0,slash.start)+token+value.slice(slash.end));setSlash(null);editor.current?.focus();
  }
  function keyDown(event:KeyboardEvent<HTMLDivElement>) {
    if (event.nativeEvent.isComposing || disabled)return;
    if ((event.metaKey||event.ctrlKey)&&event.key.toLowerCase()==="z") {event.preventDefault();const source=event.shiftKey?redo:undo,target=event.shiftKey?undo:redo;const next=source.current.pop();if(next!==undefined){target.current.push(value);pendingCaret.current=next.length;onChange(next);}return;}
    if (slash) {
      if (event.key==="Escape"){event.preventDefault();setSlash(null);return;}
      if (["ArrowUp","ArrowDown"].includes(event.key)){event.preventDefault();setActive(i=>choices.length?(i+(event.key==="ArrowDown"?1:-1)+choices.length)%choices.length:0);return;}
      if (event.key==="Enter"||event.key==="Tab"){event.preventDefault();if(choices[active])select(choices[active]);return;}
    }
    const selection=window.getSelection(),position=caret(event.currentTarget);
    if (selection?.isCollapsed!==false && history.length && ((event.key==="ArrowUp"&&!value.slice(0,position).includes("\n"))||(event.key==="ArrowDown"&&recall.current.index>=0&&!value.slice(position).includes("\n")))) {
      event.preventDefault();const saved=recall.current;
      if(event.key==="ArrowUp"){if(saved.index===-1)saved.draft=value;saved.index=Math.min(history.length-1,saved.index+1);}else saved.index--;
      const next=saved.index<0?saved.draft:history[history.length-1-saved.index];pendingCaret.current=event.key==="ArrowUp"?0:next.length;onChange(next);return;
    }
    if(event.key==="Enter"&&!event.shiftKey&&onSubmit){event.preventDefault();onSubmit();}
  }
  return <div className="themis-prompt-input">
    <div ref={editor} id={id} role="textbox" aria-label={label} aria-multiline="true" aria-disabled={disabled} aria-autocomplete="list" aria-controls={slash?listId:undefined} aria-activedescendant={slash&&choices[active]?`${listId}-${active}`:undefined} contentEditable={!disabled} suppressContentEditableWarning className="themis-prompt-editor" data-placeholder={placeholder} onFocus={detectSlash} onKeyUp={event=>{if(!["ArrowUp","ArrowDown","Enter","Tab","Escape"].includes(event.key))detectSlash();}} onKeyDown={keyDown} onInput={()=>{if(editor.current){update(serialize(editor.current));recall.current.index=-1;detectSlash();}}} onPaste={event=>{event.preventDefault();const text=event.clipboardData.getData("text/plain");document.execCommand("insertText",false,text);if(editor.current){update(serialize(editor.current));detectSlash();}}} onCopy={event=>{const selection=window.getSelection();if(selection?.rangeCount){event.clipboardData.setData("text/plain",serialize(selection.getRangeAt(0).cloneContents()));event.preventDefault();}}} />
    {slash&&<div className="themis-prompt-menu" id={listId} role="listbox" aria-label="Skills">
      {choices.length===0?<p>No matching skills</p>:choices.map((skill,index)=><button key={skill.id} type="button" id={`${listId}-${index}`} role="option" aria-selected={index===active} onMouseDown={event=>event.preventDefault()} onClick={()=>select(skill)}><SkillIcon /><span>{skill.name}<small>{skill.plugin} · {skill.description}</small></span></button>)}
    </div>}
  </div>;
}
