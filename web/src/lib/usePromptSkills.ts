import { useEffect, useRef, useState } from "react";
import { listPromptSkills, pluginAction } from "./tauri";
import type { PromptPlugin, PromptSkill } from "./prompt";
import type { Plugin } from "./types";
import { isPluginPackage } from "./integrations";

function promptPlugin(bundle:Plugin):PromptPlugin {
  const spec=bundle.spec;
  let icon:string|undefined;
  if (spec.icon?.startsWith("https://")) icon=spec.icon;
  else if (spec.icon && spec.files[spec.icon]?.trim().startsWith("<svg")) icon=`data:image/svg+xml,${encodeURIComponent(spec.files[spec.icon])}`;
  return {id:`${bundle.scope==="local"?"l":"g"}--${spec.name}`,name:spec.name,description:spec.description,origin:`${bundle.scope==="local"?"Project":"Global"} · ${spec.origin?.kind ?? "personal"}`,icon,enabled:bundle.enabled};
}

export function usePromptSkills(projectRoot?:string|null, refresh?:unknown) {
  const [skills,setSkills]=useState<PromptSkill[]>([{id:"create-skill",name:"Create skill",description:"Ask the agent to create a local or global skill",plugin:"Themis"}]);
  const [error,setError]=useState("");
  const [plugins,setPlugins]=useState<PromptPlugin[]>([]);
  const previousRoot=useRef(projectRoot);
  useEffect(()=>{
    let active=true, generation=0;
    if (previousRoot.current!==projectRoot) {
      setPlugins([]);setSkills([]);previousRoot.current=projectRoot;
    }
    const load=async()=>{
      const request=++generation;
      try {
        const [items,bundles]=await Promise.all([listPromptSkills(projectRoot),pluginAction<Plugin[]>({action:"list",projectRoot:projectRoot ?? null})]);
        if (!active || request!==generation) return;
        setSkills(items);
        setPlugins(bundles.filter(isPluginPackage).map(promptPlugin));
        setError("");
      } catch(error) {if(active && request===generation)setError(String(error));}
    };
    void load();window.addEventListener("themis-plugins-changed",load);
    return()=>{active=false;window.removeEventListener("themis-plugins-changed",load);};
  },[projectRoot,refresh]);
  return {skills,plugins,error};
}
