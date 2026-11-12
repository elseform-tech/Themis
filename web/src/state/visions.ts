// Visions in the app's state: the Visions, each open project's channels, the newest page of each
// channel's log and the work chats Visions own, kept up to date from `visions-changed` (reading
// again only what the change names), plus the cloud's moods and what needs the person.
import type { ThreadEventEnvelope, ThreadInfo } from "../lib/types";
import type { ChannelInfo, ChannelLine, ChannelPage, OwnedChat, VisionInfo, VisionsChange } from "../lib/visions";
import type { AppAction, AppState } from "./reducer/model";

export type Mood = "needs" | "sorry" | "work" | "think" | "calm" | "sleep";

export const MOOD_LABEL: Record<Mood, string> = {
  needs: "Needs you",
  sorry: "Sorry",
  work: "Working",
  think: "Thinking",
  calm: "",
  sleep: "Asleep",
};

/** Most urgent first; `sleep` is apart (the rail sleeps only when every Vision does). */
const URGENCY: Mood[] = ["needs", "sorry", "work", "think", "calm"];

export interface VisionsState {
  list: VisionInfo[];
  /** Each open project's channels, by project root. */
  channels: Record<string, ChannelInfo[]>;
  /** Each channel's log as read: the newest page and any older pages read since. */
  lines: Record<string, ChannelLine[]>;
  /** Where each channel's log continues back (null at its start). */
  nextBefore: Record<string, number | null>;
  /** Each channel's work chats, with their owners. */
  chats: Record<string, OwnedChat[]>;
  /** The open questions of the chats a channel passed questions on from, by chat, as last read. */
  questions: Record<string, string[]>;
}

export const initialVisions: VisionsState = { list: [], channels: {}, lines: {}, nextBefore: {}, chats: {}, questions: {} };

export type VisionsAction =
  | { type: "visions/loaded"; visions?: VisionInfo[]; projectRoot?: string; channels?: ChannelInfo[] }
  | { type: "visions/changed"; channelId: string; page: ChannelPage; chats: OwnedChat[]; questions: Record<string, string[]> };

/** A Vision's mood: asleep while paused or over budget; otherwise needs > sorry > work > think > calm.
 *  `chats` are its work chats, `waiting` what waits on the person from them, `runningSelf` whether
 *  its own run is going. */
export function visionMood(v: VisionInfo, chats: ThreadInfo[], waiting: number, runningSelf: boolean): Mood {
  if (v.paused || v.over_budget) return "sleep";
  if (waiting > 0) return "needs";
  if (chats.some(chat => !chat.running && chat.last_outcome === "failed")) return "sorry";
  if (chats.some(chat => chat.running)) return "work";
  return runningSelf ? "think" : "calm";
}

/** The rail cloud's mood: the most urgent one, asleep only when every Vision is. */
export function railMood(moods: Mood[]): Mood {
  if (moods.length > 0 && moods.every(mood => mood === "sleep")) return "sleep";
  return URGENCY.find(mood => moods.includes(mood)) ?? "calm";
}

function allChats(state: AppState): OwnedChat[] {
  return Object.values(state.visions.chats).flat();
}

/** Whether a passed-on question is still open: its chat's loaded history says, else the chat's
 *  questions as last read (an answer arriving live drops it there too). */
function questionOpen(state: AppState, threadId: string, questionId: string): boolean {
  if (state.historyLoaded[threadId]) {
    const row = (state.messages[threadId] ?? []).find(r => r.question?.id === questionId && !r.answeredHere);
    if (row?.question) return row.question.answer === undefined;
  }
  return state.visions.questions[threadId]?.includes(questionId) ?? false;
}

/** What waits on the person, by Vision id: approvals on its chats and its open passed-on questions. */
function waitingByVision(state: AppState): Map<string, number> {
  const owners = new Map<string, string>();
  for (const chat of allChats(state)) owners.set(chat.id, chat.owner.vision_id);
  for (const vision of state.visions.list) for (const own of vision.vision_chats) owners.set(own.thread_id, vision.id);
  const waiting = new Map<string, number>();
  const add = (visionId: string) => waiting.set(visionId, (waiting.get(visionId) ?? 0) + 1);
  for (const request of state.approvals) {
    const visionId = owners.get(request.thread_id);
    if (visionId !== undefined) add(visionId);
  }
  const counted = new Set<string>();
  for (const lines of Object.values(state.visions.lines)) {
    for (const { entry } of lines) {
      if (entry.type !== "passed_on") continue;
      const key = `${entry.thread_id}\n${entry.question_id}`;
      if (counted.has(key) || !questionOpen(state, entry.thread_id, entry.question_id)) continue;
      counted.add(key);
      add(entry.vision_id);
    }
  }
  return waiting;
}

/** Pending approvals on chats Visions own plus passed-on questions still open. */
export function needsCount(state: AppState): number {
  let total = 0;
  for (const count of waitingByVision(state).values()) total += count;
  return total;
}

/** Every Vision's mood now, by id, from live run state. */
export function visionMoods(state: AppState): Record<string, Mood> {
  const waiting = waitingByVision(state);
  const running = (id: string, listed: boolean) => state.running[id] ?? listed;
  const moods: Record<string, Mood> = {};
  for (const vision of state.visions.list) {
    const chats = allChats(state)
      .filter(chat => chat.owner.vision_id === vision.id)
      .map(chat => ({ ...chat, running: running(chat.id, chat.running) }));
    const self = vision.vision_chats.some(own => running(own.thread_id, own.running));
    moods[vision.id] = visionMood(vision, chats, waiting.get(vision.id) ?? 0, self);
  }
  return moods;
}

/** The newest page joined to the older entries already read, when nothing lies between them. */
function joinPage(old: ChannelLine[], oldNext: number | null, page: ChannelPage): { lines: ChannelLine[]; next: number | null } {
  const first = page.entries[0]?.seq;
  const older = first === undefined ? [] : old.filter(line => line.seq < first);
  const last = older[older.length - 1];
  if (last !== undefined && last.seq + 1 >= (page.next_before ?? 0)) return { lines: [...older, ...page.entries], next: oldNext };
  return { lines: page.entries, next: page.next_before };
}

function without<T>(record: Record<string, T>, keys: string[]): Record<string, T> {
  const next = { ...record };
  for (const key of keys) delete next[key];
  return next;
}

function loaded(state: AppState, action: Extract<VisionsAction, { type: "visions/loaded" }>): AppState {
  let visions = action.visions ? { ...state.visions, list: action.visions } : state.visions;
  if (action.projectRoot !== undefined && action.channels) {
    const kept = new Set(action.channels.map(channel => channel.id));
    const gone = (visions.channels[action.projectRoot] ?? []).map(channel => channel.id).filter(id => !kept.has(id));
    visions = {
      ...visions,
      channels: { ...visions.channels, [action.projectRoot]: action.channels },
      lines: without(visions.lines, gone),
      nextBefore: without(visions.nextBefore, gone),
      chats: without(visions.chats, gone),
    };
  }
  return { ...state, visions };
}

/** The visions/* actions; null for any other. */
export function reduceVisionsActions(state: AppState, action: AppAction): AppState | null {
  if (action.type === "visions/loaded") return loaded(state, action);
  if (action.type !== "visions/changed") return null;
  const { channelId, page } = action;
  const joined = joinPage(state.visions.lines[channelId] ?? [], state.visions.nextBefore[channelId] ?? null, page);
  return {
    ...state,
    visions: {
      ...state.visions,
      lines: { ...state.visions.lines, [channelId]: joined.lines },
      nextBefore: { ...state.visions.nextBefore, [channelId]: joined.next },
      chats: { ...state.visions.chats, [channelId]: action.chats },
      questions: { ...state.visions.questions, ...action.questions },
    },
  };
}

/** An answer arriving in a chat closes it among the questions read for the channels. */
export function visionsAnswered(state: AppState, envelope: ThreadEventEnvelope): AppState {
  const { thread_id: threadId, event } = envelope;
  const open = state.visions.questions[threadId];
  if (event.kind !== "question_answered" || !open?.includes(event.id)) return state;
  return { ...state, visions: { ...state.visions, questions: { ...state.visions.questions, [threadId]: open.filter(id => id !== event.id) } } };
}

/** The server calls the Visions state reads. */
export interface VisionsBridge {
  listVisions(): Promise<VisionInfo[]>;
  listChannels(projectRoot: string): Promise<ChannelInfo[]>;
  getChannelPage(channelId: string): Promise<ChannelPage>;
  listChannelThreads(channelId: string): Promise<OwnedChat[]>;
  listQuestions(threadId: string): Promise<Array<{ id: string; answer?: string }>>;
}

/** One channel read again: its newest page, its chats and the open questions of the chats its
 *  page passed questions on from (a chat gone has none). */
async function readChannel(bridge: VisionsBridge, channelId: string): Promise<VisionsAction> {
  const [page, chats] = await Promise.all([bridge.getChannelPage(channelId), bridge.listChannelThreads(channelId)]);
  const passedFrom = new Set(page.entries.flatMap(({ entry }) => entry.type === "passed_on" ? [entry.thread_id] : []));
  const questions: Record<string, string[]> = {};
  await Promise.all([...passedFrom].map(async threadId => {
    const listed = await bridge.listQuestions(threadId).catch(() => []);
    questions[threadId] = listed.filter(question => question.answer === undefined || question.answer === null).map(question => question.id);
  }));
  return { type: "visions/changed", channelId, page, chats, questions };
}

/** Reads again what `change` names: a channel (its project's channel list, then its log and
 *  chats), a Vision (the Visions, then the channels it is in), or with neither everything for the
 *  projects in `roots` and those already read. */
export async function reloadVisions(bridge: VisionsBridge, change: VisionsChange, state: AppState, dispatch: (action: VisionsAction) => void, roots: string[] = []): Promise<void> {
  const known = state.visions.channels;
  const rootOf = (id: string) => Object.keys(known).find(root => known[root]!.some(channel => channel.id === id));
  let listRoots: string[];
  if (change.channel_id !== undefined) {
    const root = rootOf(change.channel_id);
    // A channel not read yet is new: its project is one of those read.
    listRoots = root !== undefined ? [root] : Object.keys(known);
  } else if (change.vision_id !== undefined) {
    const id = change.vision_id;
    listRoots = Object.keys(known).filter(root => known[root]!.some(channel => channel.members.includes(id)));
  } else {
    listRoots = [...new Set([...Object.keys(known), ...roots])];
  }
  if (change.channel_id === undefined) dispatch({ type: "visions/loaded", visions: await bridge.listVisions() });
  const listed = await Promise.all(listRoots.map(async projectRoot => ({ projectRoot, channels: await bridge.listChannels(projectRoot) })));
  for (const { projectRoot, channels } of listed) dispatch({ type: "visions/loaded", projectRoot, channels });
  const present = listed.flatMap(entry => entry.channels);
  const toRead = change.channel_id !== undefined
    ? present.filter(channel => channel.id === change.channel_id)
    : change.vision_id !== undefined ? present.filter(channel => channel.members.includes(change.vision_id!)) : present;
  for (const action of await Promise.all(toRead.map(channel => readChannel(bridge, channel.id)))) dispatch(action);
}

const TERMINAL = new Set(["finished", "incomplete", "failed"]);

/** Follows Visions for the app: changes read one after another (a later read never lands under
 *  an earlier one), and a work chat's run ending reads its Vision again (its chats' outcomes and
 *  its budget). Failed reads leave what was shown; the next change reads again. */
export function followVisions(bridge: VisionsBridge, getState: () => AppState, dispatch: (action: VisionsAction) => void, isCancelled: () => boolean) {
  let queue: Promise<void> = Promise.resolve();
  const changed = (change: VisionsChange, roots: string[] = []) => {
    queue = queue
      .then(() => isCancelled() ? undefined : reloadVisions(bridge, change, getState(), action => { if (!isCancelled()) dispatch(action); }, roots))
      .catch(() => undefined);
    return queue;
  };
  const threadEvent = (envelope: ThreadEventEnvelope) => {
    if (!TERMINAL.has(envelope.event.kind)) return;
    const chat = allChats(getState()).find(owned => owned.id === envelope.thread_id);
    if (chat) void changed({ vision_id: chat.owner.vision_id });
  };
  return { changed, threadEvent };
}
