import {expect,it,vi} from "vitest";
import {invoke} from "@tauri-apps/api/core";
import {sendMessage,setThreadApprovalMode} from "./tauri";
import type {ThreadInfo} from "./types";
vi.mock("@tauri-apps/api/core",()=>({invoke:vi.fn(),convertFileSrc:vi.fn()}));
vi.mock("@tauri-apps/api/event",()=>({listen:vi.fn()}));
it("serializes pending permission changes and sends after the latest persistence across component mounts",async()=>{
  const finish:Array<(value:ThreadInfo)=>void>=[];
  vi.mocked(invoke).mockImplementation(async (_command,args)=> {
    if((args as {command?:string})?.command==="set_thread_approval_mode")return new Promise(resolve=>finish.push(resolve as (value:ThreadInfo)=>void));
    return {run_id:"run"};
  });
  const first=setThreadApprovalMode("thread","yolo");
  const second=setThreadApprovalMode("thread","custom");
  const send=sendMessage("thread","hello");
  await vi.waitFor(()=>expect(finish).toHaveLength(1));
  expect(invoke).not.toHaveBeenCalledWith("backend_command",expect.objectContaining({command:"send_message"}));
  finish[0]({id:"thread",approval_mode:"yolo"} as ThreadInfo);await first;
  await vi.waitFor(()=>expect(finish).toHaveLength(2));
  expect(invoke).not.toHaveBeenCalledWith("backend_command",expect.objectContaining({command:"send_message"}));
  finish[1]({id:"thread",approval_mode:"custom"} as ThreadInfo);await second;await send;
  expect(invoke).toHaveBeenLastCalledWith("backend_command",expect.objectContaining({command:"send_message"}));
});
