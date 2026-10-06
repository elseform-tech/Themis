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
  const next=bundle("discovered-hash");next.spec.icon="javascript:alert(1)";
  next.spec.origin={kind:"discovered",location:"/new/.agents/skills/Readable skill/"};
  api.pluginAction.mockResolvedValue([next]);
  const {result,rerender}=renderHook(({root})=>usePromptSkills(root),{initialProps:{root:"/old"}});
  rerender({root:"/new"});
  await waitFor(()=>expect(result.current.plugins[0]?.name).toBe("Readable skill"));
  await act(async()=>resolveOld([bundle("old")]));
  expect(result.current.plugins[0].name).toBe("Readable skill");
  expect(result.current.plugins[0].id).toBe("l--discovered-hash");
  expect(result.current.plugins[0].icon).toBeUndefined();
});
