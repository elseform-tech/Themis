import { describe, expect, it } from "vitest";
import { groupByProvider, usesGo } from "./models";
import type { GoModel } from "./types";

const models: GoModel[] = [
  { id: "local/a", effort_levels: [], provider: "local" },
  { id: "glm-5.3", effort_levels: [] },
  { id: "other/b", effort_levels: [], provider: "other" },
  { id: "local/c", effort_levels: [], provider: "local" },
];

describe("models from several providers", () => {
  it("groups OpenCode Go first, then each provider in order", () => {
    expect(groupByProvider(models).map(group => [group.provider, group.models.map(model => model.id)])).toEqual([
      [null, ["glm-5.3"]],
      ["local", ["local/a", "local/c"]],
      ["other", ["other/b"]],
    ]);
    expect(groupByProvider([models[0]]).map(group => group.provider)).toEqual(["local"]);
  });

  it("knows which chats need the OpenCode Go key", () => {
    expect(usesGo("glm-5.3", models)).toBe(true);
    expect(usesGo("local/a", models)).toBe(false);
    // Before the models load, a chat is treated as OpenCode Go's.
    expect(usesGo("local/a", [])).toBe(true);
  });
});
