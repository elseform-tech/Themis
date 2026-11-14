// @vitest-environment jsdom
// The cast (board ChiefMoods): seven presets each with their own colour and accessory, any
// shape + colour + accessory combination renders, and a state maps to one labelled expression.
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ADMIN_VISION_ID } from "../../lib/visions";
import { ACCESSORIES, COLORS, PRESETS, SHAPES, lookOf } from "./cast";
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
    expect(new Set(PRESETS.map(preset => preset.look.shape)).size).toBe(7);
    expect(PRESETS.map(preset => preset.look.accessory)).toEqual(["clipboard", "crown", "hard_hat", "glasses", "goggles", "headphones", "magnifier"]);
    expect(Object.keys(ACCESSORIES).sort()).toEqual(["clipboard", "crown", "glasses", "goggles", "hard_hat", "headphones", "magnifier", "none"]);
    for (const { look } of PRESETS) {
      expect(SHAPES[look.shape]).toBeDefined();
      expect(COLORS[look.color]).toBeDefined();
    }
  });

  it("any_shape_colour_and_accessory_combination_renders", () => {
    for (const shape of Object.keys(SHAPES)) for (const color of Object.keys(COLORS)) for (const accessory of Object.keys(ACCESSORIES)) {
      const { container, unmount } = render(<CharacterAvatar look={{ shape, color, accessory }} size={24} />);
      expect(container.querySelector("svg")?.getAttribute("data-accessory")).toBe(accessory);
      unmount();
    }
  });

  it("juno_and_atlas_are_pinned_to_the_chiefmoods_board", () => {
    // Golden values copied from ChiefMoods.dc.html (renderVals), not derived from cast.ts.
    const board = {
      juno: { body: "M12 40a8 8 0 0 1 0-16H36a8 8 0 0 1 0 16ZM7 23a8 8 0 1 1 16 0a8 8 0 1 1 -16 0ZM14 16a11 11 0 1 1 22 0a11 11 0 1 1 -22 0ZM28 23a7 7 0 1 1 14 0a7 7 0 1 1 -14 0Z", ramp: ["#D4F0FF", "#5DBDF6", "#1A74BE"], solid: "M36 28h9a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-9a2 2 0 0 1-2-2v-12a2 2 0 0 1 2-2Z", eyes: { l: 17.5, r: 27, y: 29 } },
      atlas: { body: "M12 40a8 8 0 0 1 0-16H36a8 8 0 0 1 0 16ZM7 22a9 9 0 1 1 18 0a9 9 0 1 1 -18 0ZM13.5 17a10.5 10.5 0 1 1 21 0a10.5 10.5 0 1 1 -21 0ZM24.5 22a8.5 8.5 0 1 1 17 0a8.5 8.5 0 1 1 -17 0Z", ramp: ["#E6ECF7", "#93A2BE", "#4E5B76"], solid: "M15 9V-3l4.5 5 4.5-7 4.5 7 4.5-5V9Z", eyes: { l: 18.5, r: 29.5, y: 29.5 } },
    };
    for (const [name, golden] of Object.entries(board)) {
      const preset = PRESETS.find(each => each.name.toLowerCase() === name)!;
      const shape = SHAPES[preset.look.shape]!;
      const color = COLORS[preset.look.color]!;
      expect(shape.body).toBe(golden.body);
      expect([color.hi, color.mid, color.lo]).toEqual(golden.ramp);
      expect(ACCESSORIES[preset.look.accessory]!.solid).toBe(golden.solid);
      expect(shape.eyes).toEqual(golden.eyes);
    }
  });

  it("a_small_avatar_draws_its_body_colour_and_accessory", () => {
    for (const { look } of PRESETS) {
      const { container, unmount } = render(<CharacterAvatar look={look} />);
      const paths = [...container.querySelectorAll("path")].map(path => [path.getAttribute("d"), path.getAttribute("fill")]);
      expect(paths).toContainEqual([SHAPES[look.shape]!.body, COLORS[look.color]!.flat]);
      expect(paths).toContainEqual([ACCESSORIES[look.accessory]!.solid, ACCESSORIES[look.accessory]!.fill]);
      unmount();
    }
    const { container } = render(<CharacterAvatar look={{ shape: "tall", color: "blue", accessory: "none" }} />);
    expect([...container.querySelectorAll("path")].map(path => path.getAttribute("d"))).not.toContain(ACCESSORIES.crown!.solid);
  });

  it("the_still_picture_draws_the_shaded_body_and_the_accessory", () => {
    const atlas = PRESETS[1]!.look;
    const { container } = render(<StillPicture look={atlas} expression="calm" />);
    const stops = [...container.querySelectorAll("radialGradient stop")].map(stop => stop.getAttribute("stop-color"));
    expect(stops).toEqual(["#E6ECF7", "#93A2BE", "#4E5B76"]);
    const drawn = [...container.querySelectorAll("path")].map(path => path.getAttribute("d"));
    expect(drawn).toContain(SHAPES.broad!.body);
    expect(drawn).toContain("M15 9V-3l4.5 5 4.5-7 4.5 7 4.5-5V9Z");
  });

  it("an_unknown_look_falls_back_to_the_first_shape_colour_and_no_accessory", () => {
    const { container } = render(<CharacterAvatar look={{ shape: "x", color: "y", accessory: "z" }} />);
    expect(container.querySelector("svg")?.getAttribute("data-accessory")).toBe("none");
  });

  it("the_admin_always_wears_the_crown", () => {
    expect(lookOf({ id: ADMIN_VISION_ID, shape: "broad", color: "slate" }).accessory).toBe("crown");
    expect(lookOf({ id: "rook", shape: "flat", color: "teal", accessory: "hard_hat" }).accessory).toBe("hard_hat");
    expect(lookOf({ id: "rook", shape: "flat", color: "teal" }).accessory).toBe("none");
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
