import { useEffect, useState } from "react";
import { listPromptSkills } from "./tauri";
import type { PromptSkill } from "./prompt";

export function usePromptSkills(projectRoot?:string|null, refresh?:unknown) {
  const [skills,setSkills]=useState<PromptSkill[]>([{id:"create-skill",name:"Create skill",description:"Ask the agent to create a local or global skill",plugin:"Themis"}]);
  const [error,setError]=useState("");
  useEffect(()=>{let active=true;const load=()=>listPromptSkills(projectRoot).then(items=>{if(active){setSkills(items);setError("");}}).catch(error=>{if(active)setError(String(error));});void load();window.addEventListener("themis-plugins-changed",load);return()=>{active=false;window.removeEventListener("themis-plugins-changed",load);};},[projectRoot,refresh]);
  return {skills,error};
}
