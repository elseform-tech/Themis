import { describe, expect, it } from "vitest";
import type { ChannelInfo } from "../../lib/visions";
import { initialState, type AppState } from "../../state/reducer";
import { projectChannels } from "./channels";

const channel = (id: string, name: string, members: string[]): ChannelInfo => ({
  id, name, topic: "", project_root: "/p", full_access: false, members, chief: members[0] ?? "", revision: "r",
});

describe("projectChannels", () => {
  it("finds the server's # administration and keeps it out of the other channels", () => {
    const base = initialState;
    const state: AppState = {
      ...base,
      activeProjectRoot: "/p",
      visions: { ...base.visions, channels: { "/p": [channel("c1", "release", ["juno"]), channel("a1", "administration", ["admin"])] } },
    };
    const listed = projectChannels(state);
    expect(listed.hq.id).toBe("a1");
    expect(listed.channels.map(each => each.id)).toEqual(["c1"]);
    expect(listed.current?.id).toBe("a1");
  });
});
