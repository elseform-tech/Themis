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
