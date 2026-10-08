// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { usePromptSkills } from "./usePromptSkills";
import type { Plugin } from "./types";
const api=vi.hoisted(()=>({listPromptSkills:vi.fn(),pluginAction:vi.fn()}));
vi.mock("./tauri",()=>api);
afterEach(()=>{cleanup();vi.resetAllMocks();});
function bundle(name:string,enabled=true):Plugin {
  return {scope:"local",revision:"v1",enabled,source:null,spec:{name,description:"Useful tools",version:"1",skills:[],mcp:{},hooks:[],files:{"logo.svg":"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"},icon:"logo.svg",unsupported:[],origin:{kind:"repository",location:"https://example.com/repo"}}};
}
it("loads project-scoped plugin identities and icons without embedding instructions and refreshes changes",async()=>{
  api.listPromptSkills.mockResolvedValue([]);
  api.pluginAction.mockResolvedValue([bundle("drive"),bundle("disabled",false)]);
  const {result}=renderHook(()=>usePromptSkills("/repo"));
  await waitFor(()=>expect(result.current.plugins).toHaveLength(2));
  expect(api.pluginAction).toHaveBeenCalledWith({action:"list",projectRoot:"/repo"});
  expect(result.current.plugins[0]).toMatchObject({id:"l--drive",origin:"Project · repository",enabled:true});
  expect(result.current.plugins[0].icon).toMatch(/^data:image\/svg\+xml,/);
  expect(result.current.plugins[0]).not.toHaveProperty("skills");
  api.pluginAction.mockResolvedValue([]);
  act(()=>window.dispatchEvent(new Event("themis-plugins-changed")));
  await waitFor(()=>expect(result.current.plugins).toEqual([]));
});
it("rejects stale project results and unsafe icon schemes",async()=>{
  api.listPromptSkills.mockResolvedValue([]);
  let resolveOld!:(value:Plugin[])=>void;
  api.pluginAction.mockImplementationOnce(()=>new Promise<Plugin[]>(resolve=>{resolveOld=resolve;}));
  const next=bundle("new");next.spec.icon="javascript:alert(1)";
  api.pluginAction.mockResolvedValue([next]);
  const {result,rerender}=renderHook(({root})=>usePromptSkills(root),{initialProps:{root:"/old"}});
  rerender({root:"/new"});
  await waitFor(()=>expect(result.current.plugins[0]?.name).toBe("new"));
  await act(async()=>resolveOld([bundle("old")]));
  expect(result.current.plugins[0].name).toBe("new");
  expect(result.current.plugins[0].id).toBe("l--new");
  expect(result.current.plugins[0].icon).toBeUndefined();
});
it("offers actual plugins for @ while keeping standalone skills available only through /",async()=>{
  const skill={id:"review",name:"Review",description:"Review code",instructions:"Do not eagerly inject",allowedTools:[],scripts:[]};
  const actual=bundle("one-skill-plugin");actual.spec.package_kind="plugin";actual.spec.skills=[skill];
  const imported=bundle("imported-skill");imported.spec.package_kind="skill";imported.spec.skills=[skill];
  const discovered=bundle("discovered-hash");discovered.source="discovered";discovered.spec.skills=[skill];
  const mcp=bundle("standalone-mcp");mcp.spec.package_kind="mcp";
  const hook=bundle("standalone-hook");hook.spec.package_kind="hook";
  const slashSkill={id:"l--imported-skill--review",name:"Review",description:"Review code",plugin:"imported-skill"};
  api.listPromptSkills.mockResolvedValue([slashSkill]);
  api.pluginAction.mockResolvedValue([actual,imported,discovered,mcp,hook,bundle("legacy-plugin")]);
  const {result}=renderHook(()=>usePromptSkills("/repo"));
  await waitFor(()=>expect(result.current.skills).toEqual([slashSkill]));
  expect(result.current.plugins.map(plugin=>plugin.name)).toEqual(["one-skill-plugin","legacy-plugin"]);
});

it("keeps reference labels during same-project refresh but clears them when switching projects",async()=>{
  const skill={id:"review",name:"Review",description:"Review",plugin:"personal"};
  api.listPromptSkills.mockResolvedValue([skill]);api.pluginAction.mockResolvedValue([bundle("personal")]);
  const {result,rerender}=renderHook(({root,refresh})=>usePromptSkills(root,refresh),{initialProps:{root:"/repo",refresh:0}});
  await waitFor(()=>expect(result.current.skills).toEqual([skill]));
  api.listPromptSkills.mockImplementation(()=>new Promise(()=>{}));
  rerender({root:"/repo",refresh:1});
  expect(result.current.skills).toEqual([skill]);
  expect(result.current.plugins[0].name).toBe("personal");
  rerender({root:"/other",refresh:1});
  expect(result.current.skills).toEqual([]);expect(result.current.plugins).toEqual([]);
});
