// Visions: the person's bridge to Visions, channels and the chats Visions own, with their wire
// types. Each type mirrors its Rust struct's serde form (snake_case) field for field; the shared
// fixture `visions.fixture.json` holds one of each, checked against Rust and these types.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { Question, ThreadInfo } from "./types";

/** The Visions methods of the local server, through the app. */
function invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  return tauriInvoke<T>("backend_command", { command, args });
}

export type Autonomy = "ask" | "small" | "most" | "nearly_all" | "custom";

/** The built-in admin Vision's id: Atlas, drawn with a crown, the only member of each `# hq`. */
export const ADMIN_VISION_ID = "admin";
export type ApprovalModeName = "custom" | "auto" | "yolo";

/** A Vision's own hidden chat in one of its channels (`vision_chats` on a listed Vision). */
export interface VisionChatInfo {
  thread_id: string;
  channel_id: string;
  running: boolean;
}

/** A Vision's profile (Rust `Vision`), as saved. */
export interface VisionProfile {
  /** Empty when saving a new Vision. */
  id: string;
  name: string;
  role: string;
  shape: string;
  color: string;
  model?: string;
  autonomy?: Autonomy;
  custom?: Record<string, boolean>;
  daily_budget_usd?: number;
  soul: string;
  paused: boolean;
  revision: string;
}

/** A Vision as `list_visions` gives it: its profile, whether its budget stops it spending now
 *  (also when spend cannot be checked), its own chats, what waits on the person from the chats
 *  it drives (approvals, and open questions it passed on or that ask for a permission) and whether its latest run
 *  failed with no success since. */
export interface VisionInfo extends VisionProfile {
  over_budget: boolean;
  vision_chats: VisionChatInfo[];
  needs: number;
  sorry: boolean;
}

/** A channel (Rust `Channel`). */
export interface ChannelInfo {
  id: string;
  name: string;
  topic: string;
  project_root: string;
  approval_mode: ApprovalModeName;
  members: string[];
  revision: string;
  /** In the channel list: what waits on the person in this channel's own chats (approvals and
   *  open questions passed on or asking a permission), counted by the server. */
  needs?: number;
}

export type OwnedKind = "vision_chat" | "work";
export type Hold = "none" | "stopped_by_person" | "taken_over";

/** Which Vision and channel a chat belongs to (Rust `Owner`). */
export interface OwnerInfo {
  thread_id: string;
  vision_id: string;
  channel_id: string;
  kind: OwnedKind;
  hold: Hold;
}

/** A work chat a Vision started, as `list_channel_threads` gives it. */
export type OwnedChat = ThreadInfo & { owner: OwnerInfo };

export type NoticeKind =
  | "stopped"
  | "paused"
  | "resumed"
  | "budget_reached"
  | "chat_failed"
  | "handed_back"
  | "sandbox_unavailable"
  | "budget_unknown"
  | "unpriced";

/** What the admin would set up (Rust `Proposal`): new channels and Visions by name, and changes
 *  to existing ones (a change names one `vision` or one `channel`). */
export interface Proposal {
  summary: string;
  channels: Array<{ name: string; topic: string; members: string[] }>;
  visions: Array<{ name: string; role: string; shape: string; color: string; model?: string; autonomy?: Autonomy; daily_budget_usd?: number }>;
  changes: ProposalChange[];
}
export interface ProposalChange {
  vision?: string;
  channel?: string;
  name?: string;
  role?: string;
  topic?: string;
  paused?: boolean;
  autonomy?: Autonomy;
  daily_budget_usd?: number;
  add_members?: string[];
  remove_members?: string[];
}
/** One thing a proposal set up (Rust `SetUp`). */
export interface SetUpItem {
  kind: "vision" | "channel";
  id: string;
  name: string;
  action: "created" | "changed";
}

/** One entry in a channel's log (Rust `ChannelEntry`). */
export type ChannelEntry =
  | { type: "person"; text: string }
  | { type: "vision"; vision_id: string; text: string }
  | { type: "chat_started"; vision_id: string; thread_id: string; title: string }
  | { type: "answered"; vision_id: string; thread_id: string; question_id: string; decision: string; question: string; answer: string; reason: string }
  | { type: "passed_on"; vision_id: string; thread_id: string; question_id: string; decision: string; question: string }
  | { type: "notice"; kind: NoticeKind; vision_id?: string; thread_id?: string; unpriced_requests?: number; reason?: string }
  | { type: "proposal"; vision_id: string; proposal_id: string; proposal: Proposal }
  | { type: "proposal_applied"; vision_id: string; proposal_id: string; items: SetUpItem[] };

export interface ChannelLine {
  seq: number;
  at_ms: number;
  entry: ChannelEntry;
}

/** Entries oldest first; `next_before` continues back, null at the start. */
export interface ChannelPage {
  entries: ChannelLine[];
  next_before: number | null;
}

/** Settings › Visions (`visions` in your runtime.jsonc). */
export interface VisionsSettings {
  enabled: boolean;
  autonomy: Autonomy;
  custom: Record<string, boolean>;
  daily_budget_usd: number;
  max_wakes: number;
}
export interface VisionsSettingsView {
  settings: VisionsSettings;
  revision: string;
}

/** `visions-changed`: ids only; both absent means read everything again. */
export interface VisionsChange {
  channel_id?: string;
  vision_id?: string;
}

/** A profile to save: a new one has an empty `id`. */
export type VisionDraft = Omit<VisionProfile, "id" | "revision" | "soul" | "paused" | "role"> & Partial<Pick<VisionProfile, "id" | "role" | "soul" | "paused">>;
export type ChannelDraft = Omit<ChannelInfo, "id" | "revision" | "topic"> & Partial<Pick<ChannelInfo, "id" | "topic">>;

export function listVisions(): Promise<VisionInfo[]> { return invoke("list_visions"); }
export function saveVision(vision: VisionDraft, revision: string | null): Promise<VisionProfile> { return invoke("save_vision", { vision, revision }); }
export function pauseVision(visionId: string, paused: boolean): Promise<VisionProfile> { return invoke("pause_vision", { visionId, paused }); }
export function deleteVision(visionId: string): Promise<void> { return invoke("delete_vision", { visionId }); }
export function listChannels(projectRoot: string): Promise<ChannelInfo[]> { return invoke("list_channels", { projectRoot }); }
export function saveChannel(channel: ChannelDraft, revision: string | null): Promise<ChannelInfo> { return invoke("save_channel", { channel, revision }); }
export function deleteChannel(channelId: string): Promise<void> { return invoke("delete_channel", { channelId }); }
export function getChannelPage(channelId: string, beforeSeq: number | null = null, maxBytes: number | null = null): Promise<ChannelPage> { return invoke("get_channel_page", { channelId, beforeSeq, maxBytes }); }
export function postChannelMessage(channelId: string, text: string): Promise<{ seq: number }> { return invoke("post_channel_message", { channelId, text }); }
export function listChannelThreads(channelId: string): Promise<OwnedChat[]> { return invoke("list_channel_threads", { channelId }); }
export function handBackThread(threadId: string): Promise<OwnerInfo> { return invoke("hand_back_thread", { threadId }); }
export function getVisionsSettings(): Promise<VisionsSettingsView> { return invoke("get_visions_settings"); }
export function updateVisionsSettings(changes: Partial<{ [K in keyof VisionsSettings]: VisionsSettings[K] | null }>, revision: string | null): Promise<VisionsSettingsView> { return invoke("update_visions_settings", { changes, revision }); }
/** A chat's questions, answered ones included: which passed-on questions are still open. */
export function listQuestions(threadId: string): Promise<Question[]> { return invoke("list_questions", { threadId }); }

/** Every channel entry's fields besides `type`, optional ones included: the wire fixture test
 *  checks them against Rust. */
export const ENTRY_FIELDS: { [K in ChannelEntry["type"]]: Array<Exclude<keyof Extract<ChannelEntry, { type: K }>, "type">> } = {
  person: ["text"],
  vision: ["vision_id", "text"],
  chat_started: ["vision_id", "thread_id", "title"],
  answered: ["vision_id", "thread_id", "question_id", "decision", "question", "answer", "reason"],
  passed_on: ["vision_id", "thread_id", "question_id", "decision", "question"],
  notice: ["kind", "vision_id", "thread_id", "unpriced_requests", "reason"],
  proposal: ["vision_id", "proposal_id", "proposal"],
  proposal_applied: ["vision_id", "proposal_id", "items"],
};

/** Every value of the unions the app reads, for the same check. */
export const HOLDS: Record<Hold, true> = { none: true, stopped_by_person: true, taken_over: true };
export const OWNED_KINDS: Record<OwnedKind, true> = { vision_chat: true, work: true };
export const APPROVAL_MODES: Record<ApprovalModeName, true> = { custom: true, auto: true, yolo: true };

/** Every notice kind, for the same check. */
export const NOTICE_KINDS: Record<NoticeKind, true> = {
  stopped: true, paused: true, resumed: true, budget_reached: true, chat_failed: true,
  handed_back: true, sandbox_unavailable: true, budget_unknown: true, unpriced: true,
};
