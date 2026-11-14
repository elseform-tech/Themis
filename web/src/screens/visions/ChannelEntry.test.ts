import { describe, expect, it } from "vitest";
import type { ChannelEntry, NoticeKind } from "../../lib/visions";
import { noticeText } from "./ChannelEntry";

const notice = (kind: NoticeKind, extra: Partial<Extract<ChannelEntry, { type: "notice" }>> = {}) => ({ type: "notice" as const, kind, vision_id: "v", ...extra });

describe("noticeText", () => {
  it("says each notice in plain words, with the chat after the sentence, never inside it", () => {
    const said = (kind: NoticeKind, chat: "work" | "own" | "none", extra = {}) => noticeText(notice(kind, extra), "Rook", chat);
    expect(said("stopped", "work")).toEqual({ text: "You stopped a chat Rook started", chip: true });
    expect(said("stopped", "own")).toEqual({ text: "You stopped Rook's reply.", chip: false });
    expect(said("paused", "none")).toEqual({ text: "You paused Rook.", chip: false });
    expect(said("resumed", "none")).toEqual({ text: "Rook is back.", chip: false });
    expect(said("budget_reached", "none")).toEqual({ text: "Rook reached today's budget.", chip: false });
    expect(said("budget_reached", "work")).toEqual({ text: "Rook reached today's budget", chip: true });
    expect(said("chat_failed", "work")).toEqual({ text: "A chat Rook started failed", chip: true });
    expect(said("chat_failed", "own")).toEqual({ text: "Rook couldn't finish its reply.", chip: false });
    expect(said("chat_failed", "none")).toEqual({ text: "A chat Rook started failed.", chip: false });
    expect(said("handed_back", "work")).toEqual({ text: "You handed this chat back to Rook", chip: true });
    expect(said("handed_back", "none")).toEqual({ text: "You handed a chat back to Rook.", chip: false });
    expect(said("sandbox_unavailable", "none")).toEqual({ text: "Rook couldn't run: no sandbox is available.", chip: false });
    expect(said("budget_unknown", "none")).toEqual({ text: "Rook stopped: today's spend is unknown.", chip: false });
    expect(said("unpriced", "none", { unpriced_requests: 3 })).toEqual({ text: "3 of today's requests had no price.", chip: false });
    expect(said("unpriced", "none")).toEqual({ text: "Some of today's requests had no price.", chip: false });
  });
});
