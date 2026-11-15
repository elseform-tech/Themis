// @vitest-environment jsdom
// The cast (board ChiefMoods): seven presets each with their own colour and accessory, any
// colour + accessory combination renders, and a state maps to one labelled expression.
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ADMIN_VISION_ID } from "../../lib/visions";
import { ACCESSORIES, COLORS, EYES, OUTLINE, PRESETS, lookOf } from "./cast";
import { CharacterAvatar } from "./CharacterAvatar";
import { EXPRESSIONS, EXPRESSION_LABEL, expressionFor, type ExpressionState } from "./Expression";
import { StillPicture } from "./StillPicture";

afterEach(cleanup);

const quiet: ExpressionState = { running: false, waitingOnPerson: false, finished: false, failed: false, blocked: false, paused: false, overBudget: false };

describe("the cast", () => {
  it("every_character_has_its_own_colour_and_accessory", () => {
    expect(PRESETS.map(preset => preset.name)).toEqual(["Juno", "Atlas", "Rook", "Wren", "Pip", "Sol", "Tess"]);
    expect(new Set(PRESETS.map(preset => preset.look.color)).size).toBe(7);
    expect(new Set(PRESETS.map(preset => preset.look.accessory)).size).toBe(7);
    expect(PRESETS.map(preset => preset.role)).toEqual(["Chief of staff", "Admin", "Builder", "Reviewer", "Scout", "Writer", "UI tester"]);
    for (const { look } of PRESETS) expect(Object.keys(look).sort()).toEqual(["accessory", "color"]);
    expect(PRESETS.map(preset => preset.look.accessory)).toEqual(["clipboard", "crown", "hard_hat", "glasses", "goggles", "headphones", "magnifier"]);
    expect(Object.keys(ACCESSORIES).sort()).toEqual(["clipboard", "crown", "glasses", "goggles", "hard_hat", "headphones", "magnifier", "none"]);
    for (const { look } of PRESETS) expect(COLORS[look.color]).toBeDefined();
  });

  it("any_colour_and_accessory_combination_renders", () => {
    for (const color of Object.keys(COLORS)) for (const accessory of Object.keys(ACCESSORIES)) {
      const { container, unmount } = render(<CharacterAvatar look={{ color, accessory }} size={24} />);
      expect(container.querySelector("svg")?.getAttribute("data-accessory")).toBe(accessory);
      unmount();
    }
  });

  it("the_body_is_one_outline", () => {
    // The rail icon's outline on the 48 box (Cloud3D board), its side lobes ending straight
    // down on the base so no hook shows at the bottom corners in 3D.
    const outline = "M13.2 39H35.8a7.98 7.98 0 0 0 0-15.96A12 12 0 0 0 13.2 20.2A9.4 9.4 0 0 0 13.2 39Z";
    expect(OUTLINE).toBe(outline);
    for (const { look } of PRESETS) {
      for (const view of [render(<CharacterAvatar look={look} />), render(<StillPicture look={look} expression="calm" />)]) {
        const fills = [...view.container.querySelectorAll("path")].filter(path => path.getAttribute("d") === outline);
        expect(fills.length).toBeGreaterThan(0);
        expect(view.container.querySelectorAll("circle")).toHaveLength(0);
        for (const path of view.container.querySelectorAll("path")) expect(path.getAttribute("d") ?? "").not.toMatch(/a\d+(\.\d+)? \d+(\.\d+)? 0 1 1/);
        view.unmount();
      }
    }
  });

  it("juno_and_atlas_are_pinned_to_the_cloud3d_board", () => {
    // Golden values copied from Cloud3D.dc.html (renderVals), not derived from cast.ts.
    const board = {
      juno: { solid: "M36 30h8a1.8 1.8 0 0 1 1.8 1.8v10.4a1.8 1.8 0 0 1-1.8 1.8h-8a1.8 1.8 0 0 1-1.8-1.8V31.8A1.8 1.8 0 0 1 36 30Z", flat: "#38A9F0" },
      atlas: { solid: "M17.5 16V8.6l3.2 3.6 3.3-5.6 3.3 5.6 3.2-3.6V16Z", flat: "#7C8AA5" },
    };
    for (const [name, golden] of Object.entries(board)) {
      const preset = PRESETS.find(each => each.name.toLowerCase() === name)!;
      expect(COLORS[preset.look.color]!.flat).toBe(golden.flat);
      expect(ACCESSORIES[preset.look.accessory]!.solid).toBe(golden.solid);
      expect(EYES).toEqual({ l: 19.5, r: 28.5, y: 29 });
    }
  });

  it("the_accessory_can_be_hidden", () => {
    const { container } = render(<CharacterAvatar look={PRESETS[1]!.look} accessory={false} />);
    expect([...container.querySelectorAll("path")].map(path => path.getAttribute("d"))).not.toContain(ACCESSORIES.crown!.solid);
  });

  it("a_small_avatar_draws_its_body_colour_and_accessory", () => {
    for (const { look } of PRESETS) {
      const { container, unmount } = render(<CharacterAvatar look={look} />);
      const paths = [...container.querySelectorAll("path")].map(path => [path.getAttribute("d"), path.getAttribute("fill")]);
      expect(paths).toContainEqual([OUTLINE, COLORS[look.color]!.flat]);
      expect(paths).toContainEqual([ACCESSORIES[look.accessory]!.solid, ACCESSORIES[look.accessory]!.fill]);
      unmount();
    }
    const { container } = render(<CharacterAvatar look={{ color: "blue", accessory: "none" }} />);
    expect([...container.querySelectorAll("path")].map(path => path.getAttribute("d"))).not.toContain(ACCESSORIES.crown!.solid);
  });

  it("the_still_picture_draws_the_shaded_body_and_the_accessory", () => {
    const atlas = PRESETS[1]!.look;
    const { container } = render(<StillPicture look={atlas} expression="calm" />);
    const stops = [...container.querySelectorAll("radialGradient stop")].map(stop => stop.getAttribute("stop-color"));
    expect(stops).toEqual(["#E6ECF7", "#93A2BE", "#4E5B76"]);
    const drawn = [...container.querySelectorAll("path")].map(path => path.getAttribute("d"));
    expect(drawn).toContain(OUTLINE);
    expect(drawn).toContain(ACCESSORIES.crown!.solid);
  });

  it("an_unknown_look_falls_back_to_blue_and_no_accessory", () => {
    const { container } = render(<CharacterAvatar look={{ color: "y", accessory: "z" }} />);
    expect(container.querySelector("path")?.getAttribute("fill")).toBe(COLORS.blue!.flat);
    expect(container.querySelector("svg")?.getAttribute("data-accessory")).toBe("none");
  });

  it("the_admin_always_wears_the_crown", () => {
    expect(lookOf({ id: ADMIN_VISION_ID, color: "slate" }).accessory).toBe("crown");
    expect(lookOf({ id: "rook", color: "teal", accessory: "hard_hat" }).accessory).toBe("hard_hat");
    expect(lookOf({ id: "rook", color: "teal" }).accessory).toBe("none");
  });
});

describe("the expressions", () => {
  it("each_state_maps_to_one_expression_with_a_label", () => {
    expect(expressionFor(quiet)).toBe("calm");
    expect(expressionFor({ ...quiet, running: true })).toBe("working");
    expect(expressionFor({ ...quiet, finished: true })).toBe("done");
    expect(expressionFor({ ...quiet, waitingOnPerson: true })).toBe("needs");
    expect(expressionFor({ ...quiet, failed: true })).toBe("stuck");
    expect(expressionFor({ ...quiet, blocked: true })).toBe("stuck");
    expect(expressionFor({ ...quiet, paused: true })).toBe("paused");
    expect(expressionFor({ ...quiet, overBudget: true })).toBe("paused");
    // Paused outranks everything, stuck outranks needs, needs outranks working, working outranks done.
    expect(expressionFor({ ...quiet, paused: true, running: true, failed: true })).toBe("paused");
    expect(expressionFor({ ...quiet, failed: true, waitingOnPerson: true, running: true })).toBe("stuck");
    expect(expressionFor({ ...quiet, waitingOnPerson: true, running: true })).toBe("needs");
    expect(expressionFor({ ...quiet, running: true, finished: true })).toBe("working");
    expect([...EXPRESSIONS].map(each => EXPRESSION_LABEL[each])).toEqual(["Calm", "Working", "Done", "Needs you", "Stuck", "Paused"]);
  });

  it("every_expression_draws_a_different_still_picture", () => {
    const look = PRESETS[0]!.look;
    const drawn = EXPRESSIONS.map(expression => {
      const { container, unmount } = render(<StillPicture look={look} expression={expression} />);
      const html = container.querySelector("svg g.vc")?.innerHTML ?? "";
      expect(screen.getByRole("img").getAttribute("aria-label")).toContain(EXPRESSION_LABEL[expression]);
      unmount();
      return html;
    });
    expect(new Set(drawn).size).toBe(6);
  });

  it("reduced_motion_shows_a_still_face", () => {
    const original = window.matchMedia;
    const look = PRESETS[0]!.look;
    try {
      window.matchMedia = (() => ({ matches: false })) as unknown as typeof window.matchMedia;
      const moving = render(<StillPicture look={look} expression="working" />);
      expect(moving.container.querySelector("g.vc")?.getAttribute("class")).toContain("vc-work");
      moving.unmount();
      window.matchMedia = (() => ({ matches: true })) as unknown as typeof window.matchMedia;
      const still = render(<StillPicture look={look} expression="working" />);
      expect(still.container.querySelector("g.vc")?.getAttribute("class")).not.toContain("vc-work");
      expect(still.container.querySelector("svg")?.getAttribute("data-still")).toBe("true");
    } finally { window.matchMedia = original; }
  });
});
