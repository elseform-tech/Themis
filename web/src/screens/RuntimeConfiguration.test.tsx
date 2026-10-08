// @vitest-environment jsdom
import {act,cleanup,fireEvent,render,screen} from "@testing-library/react";
import {afterEach,beforeEach,expect,it,vi} from "vitest";
import {RuntimeConfiguration,DiagnosticLogs} from "./RuntimeConfiguration";
import {getRuntimeConfiguration,saveRuntimeConfiguration,queryDiagnosticLogs} from "../lib/tauri";
vi.mock("../lib/tauri",()=>({getRuntimeConfiguration:vi.fn(),saveRuntimeConfiguration:vi.fn(),queryDiagnosticLogs:vi.fn()}));
afterEach(cleanup);
beforeEach(()=>vi.clearAllMocks());
it("loads effective sources, validates selected project JSON and preserves failed edits",async()=>{
  const view={user_path:"/user/runtime.jsonc",project_path:"/project/.themis/config.jsonc",user_json:'{"schema_version":1}',project_json:"{}",config:{approval:{default:"ask"}},provenance:{approval:"default"},schema:{type:"object"}};
  vi.mocked(getRuntimeConfiguration).mockResolvedValue(view);vi.mocked(saveRuntimeConfiguration).mockRejectedValue(new Error("approval.default: invalid action"));
  render(<RuntimeConfiguration projectRoot="/project"/>);
  await screen.findByText("/user/runtime.jsonc");
  fireEvent.change(screen.getByRole("combobox",{name:"Configuration scope"}),{target:{value:"project"}});
  fireEvent.change(screen.getByRole("textbox",{name:"Configuration JSON"}),{target:{value:'{"approval":{"default":"invalid"}}'}});
  await act(async()=>fireEvent.click(screen.getByRole("button",{name:"Validate and save"})));
  expect(saveRuntimeConfiguration).toHaveBeenCalledWith("project",'{"approval":{"default":"invalid"}}',"/project");
  expect(screen.getByRole("alert")).toHaveTextContent("approval.default");
  expect(screen.getByRole("textbox",{name:"Configuration JSON"})).toHaveValue('{"approval":{"default":"invalid"}}');
});
it("queries diagnostic filters and copies only the returned JSONL",async()=>{
  const record={timestamp:1000,level:"error",service:"plugins",event:"import.failed",details:{operation_id:"op1"}};
  vi.mocked(queryDiagnosticLogs).mockResolvedValue([record]);const writeText=vi.fn().mockResolvedValue(undefined);Object.defineProperty(navigator,"clipboard",{value:{writeText},configurable:true});
  render(<DiagnosticLogs operationId="op1"/>);expect(screen.getByRole("textbox",{name:"Log operation"})).toHaveValue("op1");await screen.findByText("1 records · up to 200 per query");fireEvent.change(screen.getByRole("combobox",{name:"Log severity"}),{target:{value:"error"}});fireEvent.change(screen.getByRole("textbox",{name:"Log service"}),{target:{value:"plugins"}});fireEvent.change(screen.getByRole("textbox",{name:"Log operation"}),{target:{value:"op1"}});
  await act(async()=>fireEvent.click(screen.getByRole("button",{name:"Load logs"})));
  expect(queryDiagnosticLogs).toHaveBeenCalledWith({service:"plugins",level:"error",operationId:"op1",limit:200});
  await act(async()=>fireEvent.click(screen.getByRole("button",{name:"Copy logs"})));
  expect(writeText).toHaveBeenCalledWith(JSON.stringify(record));expect(screen.getByRole("status")).toHaveTextContent("Copied");
});
