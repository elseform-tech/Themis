// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { keepDrafts, useDraft, useFormDraft } from "./settingsDrafts";

describe("keepDrafts", () => {
  it("takes the new saved values except draft fields the person changed", () => {
    const shown = { turns: 200, chats: 3, sound: true };
    const form = { turns: 250, chats: 3, sound: true };
    const next = { turns: 100, chats: 5, sound: false };
    expect(keepDrafts(form, shown, next, ["turns", "chats"])).toEqual({ turns: 250, chats: 5, sound: false });
  });
});

describe("useFormDraft", () => {
  it("follows saved settings and keeps an edit in progress", () => {
    const { result, rerender } = renderHook(({ saved }) => useFormDraft(saved, ["turns"]), { initialProps: { saved: { turns: 200, chats: 3 } } });
    rerender({ saved: { turns: 100, chats: 3 } });
    expect(result.current[0]).toEqual({ turns: 100, chats: 3 });
    act(() => result.current[1](form => ({ ...form, turns: 150 })));
    rerender({ saved: { turns: 120, chats: 4 } });
    expect(result.current[0]).toEqual({ turns: 150, chats: 4 });
  });
});

describe("useDraft", () => {
  it("follows the saved value until the person edits it", () => {
    const { result, rerender } = renderHook(({ saved }) => useDraft(saved), { initialProps: { saved: 600 } });
    rerender({ saved: 30 });
    expect(result.current[0]).toBe(30);
    act(() => result.current[1](45));
    rerender({ saved: 60 });
    expect(result.current[0]).toBe(45);
    // Saved as typed: it is no longer a draft, and the next change elsewhere shows again.
    rerender({ saved: 45 });
    rerender({ saved: 90 });
    expect(result.current[0]).toBe(90);
  });
});
