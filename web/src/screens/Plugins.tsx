import type { InputProps } from "../components/primitives/Input";
import { useCallback, useEffect, useState, useId } from "react";
import { Button, Dialog, Input as PrimitiveInput, EmptyState } from "../components";
import { pluginAction } from "../lib/tauri";
import type { Marketplace, Plugin, PluginSpec } from "../lib/types";
import { describeError, useApp } from "../state/store";
import { readSession, writeSession } from "../state/session";
import { Skills as PersonalSkills } from "./Skills";
import "./Skills.css";
import "./Plugins.css";

function Field(props: Omit<InputProps,"id">) { const id=useId(); return <PrimitiveInput id={id} {...props}/>; }

const events=["RunStart","BeforeToolCall","AfterToolCall","ToolCallFailed","RunFinished","BeforeCompaction"];
function nextName(prefix:string, names:string[]) { let index=1; while(names.includes(`${prefix}-${index}`))index++; return `${prefix}-${index}`; }
const emptySpec=():PluginSpec=>({name:"",description:"",version:"",skills:[],mcp:{},hooks:[],files:{},unsupported:[]});
export function Plugins() {
  const {state,dispatch}=useApp();
  const root=state.activeProjectRoot;
  const [plugins,setPlugins]=useState<Plugin[]>([]),[markets,setMarkets]=useState<Marketplace[]>([]);
  const [tab,setTab]=useState("Installed"),[error,setError]=useState(""),[busy,setBusy]=useState(false);
  const [catalog,setCatalog]=useState<{market:string;entries:Array<{name:string;description?:string}>}|null>(null);
  const [marketName,setMarketName]=useState(""),[marketSource,setMarketSource]=useState("");
  const [scope,setScope]=useState<"local"|"global">("global");
  const [editing,setEditing]=useState<{original:Plugin|null;spec:PluginSpec;scope:"local"|"global"}|null>(null);
  const [section,setSection]=useState("Skills"),[result,setResult]=useState("");
  const [removing,setRemoving]=useState<Plugin|null>(null);
  const load=useCallback(async()=>{const [items,sources]=await Promise.all([pluginAction<Plugin[]>({action:"list",projectRoot:root}),pluginAction<Marketplace[]>({action:"marketplaces"})]);setPlugins(items);setMarkets(sources);},[root]);
  useEffect(()=>{let active=true;void load().catch(e=>{if(active)setError(describeError(e));});return()=>{active=false;};},[load]);
  async function action(args:Record<string,unknown>) {
    setBusy(true);setError("");try{const value=await pluginAction<unknown>({projectRoot:root,...args});await load();window.dispatchEvent(new Event("themis-plugins-changed"));return value;}catch(e){setError(describeError(e));throw e;}finally{setBusy(false);}
  }
  function patch(patch:Partial<PluginSpec>){setEditing(current=>current?{...current,spec:{...current.spec,...patch}}:null);}
  function createSkill(){
    if(!editing)return;
    const id=state.activeThreadId;
    if(!id){setError("Open a chat in this project to ask the agent to create a skill.");return;}
    const drafts=readSession<Record<string,string>>("drafts",{});
    drafts[id]=`[[skill:create-skill]] Create a reusable skill in the ${editing.scope} plugin ${editing.spec.name || "personal"}. `;
    writeSession("drafts",drafts);window.dispatchEvent(new Event("themis-drafts-changed"));setEditing(null);dispatch({type:"ui/view",view:"thread"});
  }
  return <div className="themis-skills themis-plugins">
    <div className="themis-skills-head"><h2 className="themis-skills-title">Plugins</h2><Button onClick={()=>{setEditing({original:null,spec:emptySpec(),scope:root?"local":"global"});setSection("Skills");setResult("");}}>New plugin</Button></div>
    <div className="themis-plugins-toolbar" role="group" aria-label="Plugin views">{["Installed","Marketplaces"].map(t=><Button key={t} variant={t===tab?"primary":"ghost"} onClick={()=>setTab(t)}>{t}</Button>)}<Button variant="ghost" disabled={busy} onClick={()=>void load().catch(e=>setError(describeError(e)))}>Refresh</Button></div>
    {error&&<p role="alert" className="themis-skills-error">{error}</p>}
    {tab==="Installed"?<>
      {plugins.length===0&&<EmptyState title="No plugins installed" hint="Browse a marketplace or create a personal plugin. Use /create-skill in a chat to ask the agent to build skills."/>}
      {plugins.map(plugin=><section key={plugin.scope+plugin.spec.name} className="themis-plugin-row"><div><strong>{plugin.spec.name}</strong><p>{plugin.spec.description}</p><small>{plugin.scope} · {plugin.source ?? "Personal"} · {plugin.spec.skills.length} skills · {Object.keys(plugin.spec.mcp).length} connections · {plugin.spec.hooks.length} hooks</small></div><div className="themis-plugins-toolbar">
        <Button disabled={busy} onClick={()=>{setEditing({original:plugin,spec:structuredClone(plugin.spec),scope:plugin.scope});setSection("Skills");setResult("");}}>Manage</Button>
        <Button disabled={busy} onClick={()=>void action({action:plugin.enabled?"disable":"enable",scope:plugin.scope,name:plugin.spec.name}).catch(()=>{})}>{plugin.enabled?"Disable":"Enable"}</Button>
        {plugin.source&&<Button disabled={busy} onClick={()=>void action({action:"update",scope:plugin.scope,name:plugin.spec.name,marketplace:plugin.source}).catch(()=>{})}>Update</Button>}
        <Button variant="danger" disabled={busy} onClick={()=>setRemoving(plugin)}>Uninstall</Button>
      </div></section>)}
      {state.skills.length>0&&<details><summary>Personal skills from earlier versions</summary><PersonalSkills/></details>}
    </>:<>
      <label>Install scope <select value={scope} onChange={e=>setScope(e.target.value as "local"|"global")}><option value="global">Global · all projects</option><option value="local" disabled={!root}>Local · current project</option></select></label>
      {markets.map(market=><section className="themis-plugin-row" key={market.name}><div><strong>{market.name}</strong><p>{market.source}</p></div><div className="themis-plugins-toolbar"><Button disabled={busy} onClick={()=>void action({action:"catalog",name:market.name}).then(value=>setCatalog({market:market.name,entries:(value as {plugins:Array<{name:string;description?:string}>}).plugins})).catch(()=>{})}>Browse</Button><Button disabled={busy} onClick={()=>void action({action:"catalog",name:market.name,refresh:true}).then(value=>setCatalog({market:market.name,entries:(value as {plugins:Array<{name:string;description?:string}>}).plugins})).catch(()=>{})}>Refresh catalog</Button><Button variant="ghost" disabled={busy} onClick={()=>void action({action:"remove_marketplace",name:market.name}).then(()=>setCatalog(null)).catch(()=>{})}>Remove</Button></div></section>)}
      <form onSubmit={e=>{e.preventDefault();void action({action:"add_marketplace",name:marketName,source:marketSource}).then(()=>{setMarketName("");setMarketSource("");}).catch(()=>{});}} className="themis-plugins-form"><Field label="Marketplace name" value={marketName} onChange={e=>setMarketName(e.target.value)} required/><Field label="Repository URL or local directory" value={marketSource} onChange={e=>setMarketSource(e.target.value)} required/><Button type="submit" disabled={busy}>Add marketplace</Button></form>
      {catalog&&<div><h3>{catalog.market}</h3>{catalog.entries.map(entry=><section className="themis-plugin-row" key={entry.name}><div><strong>{entry.name}</strong><p>{entry.description}</p></div><Button disabled={busy} onClick={()=>void action({action:"install",name:entry.name,marketplace:catalog.market,scope}).catch(()=>{})}>Install</Button></section>)}</div>}
    </>}
    <Dialog open={editing!==null} title={editing?.original?`Manage ${editing.spec.name}`:"New plugin"} onClose={()=>{if(!busy)setEditing(null);}}>
      {editing&&<div className="themis-plugins-form">
        {editing.original?.source&&<p>Marketplace package. Create a personal copy to customize its skills, connections, and hooks.</p>}
        <Field label="Plugin name" value={editing.spec.name} disabled={!!editing.original} onChange={e=>patch({name:e.target.value})}/>
        <Field label="Description" value={editing.spec.description} disabled={!!editing.original?.source} onChange={e=>patch({description:e.target.value})}/>
        {!editing.original&&<label>Scope <select value={editing.scope} onChange={e=>setEditing({...editing,scope:e.target.value as "local"|"global"})}><option value="global">Global</option><option value="local" disabled={!root}>Local</option></select></label>}
        <div className="themis-plugins-toolbar" role="group" aria-label="Plugin components">{["Skills","Connections","Hooks","Advanced"].map(t=><Button key={t} variant={section===t?"primary":"ghost"} onClick={()=>setSection(t)}>{t}</Button>)}</div>
        <fieldset disabled={!!editing.original?.source || busy}>
        {section==="Skills"&&<><Button onClick={createSkill}>Ask agent to create skill</Button>{editing.spec.skills.map((skill,index)=><details key={skill.id}><summary>{skill.name}</summary><Field label="Skill name" value={skill.name} onChange={e=>patch({skills:editing.spec.skills.map((s,i)=>i===index?{...s,name:e.target.value}:s)})}/><label>Instructions<textarea value={skill.instructions} onChange={e=>patch({skills:editing.spec.skills.map((s,i)=>i===index?{...s,instructions:e.target.value}:s)})}/></label><Button variant="danger" onClick={()=>patch({skills:editing.spec.skills.filter((_,i)=>i!==index)})}>Delete skill</Button></details>)}</>}
        {section==="Connections"&&<><Button onClick={()=>{const name=nextName("connection",Object.keys(editing.spec.mcp));patch({mcp:{...editing.spec.mcp,[name]:{url:"",args:[],env:{},enabled:false}},skills:editing.spec.skills.length?editing.spec.skills:[{id:"connect",name:`Use ${editing.spec.name}`,description:"Use this plugin's connected tools",instructions:"Use the available MCP tools to complete the user's task. Respect approvals.",allowedTools:[],scripts:[]}]});}}>Add MCP server</Button>{Object.entries(editing.spec.mcp).map(([name,server])=><details key={name}><summary>{name} · {server.enabled?"Enabled":"Disabled"}</summary>
          <label>Transport<select value={server.command!=null?"local":"remote"} onChange={e=>patch({mcp:{...editing.spec.mcp,[name]:{...server,command:e.target.value==="local"?"":null,url:e.target.value==="remote"?"":null}}})}><option value="remote">Remote URL</option><option value="local">Local executable</option></select></label>
          <Field label={server.command!=null?"Executable":"Server URL"} value={server.command ?? server.url ?? ""} onChange={e=>patch({mcp:{...editing.spec.mcp,[name]:{...server,...(server.command!=null?{command:e.target.value}:{url:e.target.value})}}})}/>
          {server.command!=null&&<label>Arguments (one per line)<textarea value={server.args.join("\n")} onChange={e=>patch({mcp:{...editing.spec.mcp,[name]:{...server,args:e.target.value.split("\n").filter(Boolean)}}})}/></label>}
          {server.url!=null&&<Field label="Bearer token environment variable (optional)" value={server.bearer_env??""} onChange={e=>patch({mcp:{...editing.spec.mcp,[name]:{...server,bearer_env:e.target.value||null}}})}/>}
          <label><input type="checkbox" checked={server.enabled} onChange={e=>patch({mcp:{...editing.spec.mcp,[name]:{...server,enabled:e.target.checked}}})}/> Enabled when a skill from this plugin is used</label>
          <Button onClick={()=>void action({action:"test_mcp",server}).then(value=>setResult(JSON.stringify(value,null,2))).catch(()=>{})}>Test connection & list tools</Button><Button variant="danger" onClick={()=>patch({mcp:Object.fromEntries(Object.entries(editing.spec.mcp).filter(([key])=>key!==name))})}>Remove connection</Button>
        </details>)}</>}
        {section==="Hooks"&&<><Button onClick={()=>patch({hooks:[...editing.spec.hooks,{name:nextName("hook",editing.spec.hooks.map(h=>h.name)),event:"AfterToolCall",command:"",enabled:false,timeout_seconds:10,blocking:false}]})}>Add hook</Button>{editing.spec.hooks.map((hook,index)=><details key={index}><summary>{hook.name} · {hook.event}</summary><Field label="Name" value={hook.name} onChange={e=>patch({hooks:editing.spec.hooks.map((h,i)=>i===index?{...h,name:e.target.value}:h)})}/><label>Event<select value={hook.event} onChange={e=>patch({hooks:editing.spec.hooks.map((h,i)=>i===index?{...h,event:e.target.value}:h)})}>{events.map(event=><option key={event}>{event}</option>)}</select></label><Field label="Command" value={hook.command} onChange={e=>patch({hooks:editing.spec.hooks.map((h,i)=>i===index?{...h,command:e.target.value}:h)})}/><Field label="Timeout (seconds, 1–60)" type="number" min={1} max={60} value={hook.timeout_seconds} onChange={e=>patch({hooks:editing.spec.hooks.map((h,i)=>i===index?{...h,timeout_seconds:Number(e.target.value)}:h)})}/><label><input type="checkbox" checked={hook.enabled} onChange={e=>patch({hooks:editing.spec.hooks.map((h,i)=>i===index?{...h,enabled:e.target.checked}:h)})}/> Enabled</label><label><input type="checkbox" checked={hook.blocking} onChange={e=>patch({hooks:editing.spec.hooks.map((h,i)=>i===index?{...h,blocking:e.target.checked}:h)})}/> Block on failure at before events</label><Button onClick={()=>void action({action:"test_hook",hook}).then(value=>setResult(JSON.stringify(value,null,2))).catch(()=>{})}>Run test command</Button><Button variant="danger" onClick={()=>patch({hooks:editing.spec.hooks.filter((_,i)=>i!==index)})}>Delete hook</Button></details>)}</>}
        {section==="Advanced"&&<label>Bundle configuration<textarea defaultValue={JSON.stringify(editing.spec,null,2)} onBlur={e=>{try{const spec=JSON.parse(e.target.value) as PluginSpec; if(typeof spec.name!=="string" || !Array.isArray(spec.skills) || !Array.isArray(spec.hooks) || !spec.mcp || typeof spec.mcp!=="object" || !spec.files || !Array.isArray(spec.unsupported)) throw new Error("Invalid bundle"); patch(spec);}catch{setError("Invalid bundle JSON");}}}/></label>}
        </fieldset>
        {editing.spec.unsupported.length>0&&<div><strong>Unsupported imported components</strong><ul>{editing.spec.unsupported.map((item,i)=><li key={i}>{item}</li>)}</ul></div>}
        {result&&<pre aria-live="polite">{result}</pre>}
        {error&&<p role="alert">{error}</p>}
        <div className="themis-plugins-toolbar"><Button disabled={busy} onClick={()=>setEditing(null)}>Close</Button>{editing.original?.source?<Button onClick={()=>setEditing({original:null,scope:root?"local":"global",spec:{...structuredClone(editing.spec),name:`${editing.spec.name}-personal`}})}>Create personal copy</Button>:<Button variant="primary" disabled={busy} onClick={()=>void action({action:"save",scope:editing.scope,spec:editing.spec,expectedRevision:editing.original?.revision??null}).then(()=>setEditing(null)).catch(()=>{})}>Save changes</Button>}<Button onClick={()=>{const url=URL.createObjectURL(new Blob([JSON.stringify(editing.spec,null,2)],{type:"application/json"}));const a=document.createElement("a");a.href=url;a.download=`${editing.spec.name||"plugin"}.json`;a.click();URL.revokeObjectURL(url);}}>Export</Button></div>
      </div>}
    </Dialog>
    <Dialog open={removing!==null} title="Uninstall plugin?" onClose={()=>setRemoving(null)}><p>Saved prompts and automations referencing {removing?.spec.name} will retain unavailable skill chips and cannot run until repaired.</p><Button variant="danger" disabled={busy} onClick={()=>{if(removing)void action({action:"delete",scope:removing.scope,name:removing.spec.name}).then(()=>setRemoving(null)).catch(()=>{});}}>Uninstall</Button></Dialog>
  </div>;
}
