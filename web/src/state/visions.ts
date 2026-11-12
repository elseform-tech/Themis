// Visions in the app's state: the Visions as the server lists them, each open project's channels,
// a window of each channel's newest lines and the work chats Visions own, kept up to date from
// `visions-changed` (reading again only what the change names), plus the cloud's moods. What
// waits on the person, whether a Vision is sorry and whether its budget lets it run are the
// server's own values (`list_visions`); the app adds only which runs are going now.
import type { ThreadEventEnvelope } from "../lib/types";
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

/** How many of a channel's newest lines are kept, unless the person read further back. */
export const WINDOW_LINES = 200;

export interface VisionsState {
  list: VisionInfo[];
  /** Each open project's channels, by project root. */
  channels: Record<string, ChannelInfo[]>;
  /** Each channel's window: its newest lines, and older ones the person asked for. */
  lines: Record<string, ChannelLine[]>;
  /** Where each channel's log continues back from its window (null at its start). */
  nextBefore: Record<string, number | null>;
  /** How many lines each channel's window keeps, when the person read past `WINDOW_LINES`. */
  keep: Record<string, number>;
  /** Each channel's work chats, with their owners. */
  chats: Record<string, OwnedChat[]>;
  /** The open questions of the chats a window passed questions on from, by chat, as last read. */
  questions: Record<string, string[]>;
  /** What the Visions view shows: a channel (null is `# hq`), and a chat opened from it. */
  open: VisionsOpen;
}

export interface VisionsOpen {
  channelId: string | null;
  chatId: string | null;
  /** The plain chat that was open before a Vision's chat, for Chats to return to. */
  plainThreadId: string | null;
}

export const initialVisions: VisionsState = { list: [], channels: {}, lines: {}, nextBefore: {}, keep: {}, chats: {}, questions: {}, open: { channelId: null, chatId: null, plainThreadId: null } };

export type VisionsAction =
  | { type: "visions/loaded"; visions?: VisionInfo[]; projectRoot?: string; channels?: ChannelInfo[] }
  | { type: "visions/changed"; channelId: string; page: ChannelPage; chats: OwnedChat[]; questions: Record<string, string[]> }
  | { type: "visions/older"; channelId: string; page: ChannelPage }
  | { type: "visions/open"; channelId: string | null; chatId?: string | null }
  | { type: "visions/project"; root: string }
  | { type: "visions/reshow" };

/** A Vision's mood: asleep while paused or over budget; otherwise needs > sorry > work > think >
 *  calm. Sorry is the server's: its latest run failed (a work chat's or its own chat's; the
 *  person's Stop is none) and none of its runs has succeeded since. `working` is whether a work
 *  chat it drives runs, `thinking` whether its own chat does. */
export function visionMood(v: VisionInfo, working: boolean, thinking: boolean): Mood {
  if (v.paused || v.over_budget) return "sleep";
  if (v.needs > 0) return "needs";
  if (v.sorry) return "sorry";
  if (working) return "work";
  return thinking ? "think" : "calm";
}

/** The rail cloud's mood: the most urgent one, asleep only when every Vision is. */
export function railMood(moods: Mood[]): Mood {
  if (moods.length > 0 && moods.every(mood => mood === "sleep")) return "sleep";
  return URGENCY.find(mood => moods.includes(mood)) ?? "calm";
}

function allChats(state: AppState): OwnedChat[] {
  return Object.values(state.visions.chats).flat();
}

/** The ids of every chat a Vision runs, built once per listing of the chats (thread events leave
 *  `visions.chats` as it is, so they cost one set lookup). */
const ownedIds = new WeakMap<VisionsState["chats"], Set<string>>();
function ownedChatIds(state: AppState): Set<string> {
  let ids = ownedIds.get(state.visions.chats);
  if (!ids) {
    ids = new Set(allChats(state).map(chat => chat.id));
    ownedIds.set(state.visions.chats, ids);
  }
  return ids;
}

/** What waits on the person from every Vision, as the server counts it. */
export function needsCount(state: AppState): number {
  return state.visions.list.reduce((total, vision) => total + vision.needs, 0);
}

/** Every Vision's mood now, by id: the server's values plus which runs are going now. One pass
 *  over the work chats; no channel lines are read. */
export function visionMoods(state: AppState): Record<string, Mood> {
  const running = (id: string, listed: boolean) => state.running[id] ?? listed;
  const working = new Set<string>();
  for (const chat of allChats(state)) {
    if (chat.owner.hold !== "taken_over" && running(chat.id, chat.running)) working.add(chat.owner.vision_id);
  }
  const moods: Record<string, Mood> = {};
  for (const vision of state.visions.list) {
    const thinking = vision.vision_chats.some(own => running(own.thread_id, own.running));
    moods[vision.id] = visionMood(vision, working.has(vision.id), thinking);
  }
  return moods;
}

/** The newest page joined to the window when nothing lies between them, cut to `keep` lines.
 *  Reads at most the window and the page. */
function joinNewest(old: ChannelLine[], oldNext: number | null, keep: number, page: ChannelPage): { lines: ChannelLine[]; next: number | null } {
  const first = page.entries[0]?.seq;
  if (first === undefined) return { lines: [], next: page.next_before };
  const older = old.filter(line => line.seq < first);
  const last = older[older.length - 1];
  const joined = last !== undefined && last.seq + 1 >= (page.next_before ?? 0)
    ? { lines: [...older, ...page.entries], next: oldNext }
    : { lines: page.entries, next: page.next_before };
  const limit = Math.max(keep, page.entries.length);
  if (joined.lines.length <= limit) return joined;
  const lines = joined.lines.slice(joined.lines.length - limit);
  return { lines, next: lines[0]!.seq };
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
      keep: without(visions.keep, gone),
      chats: without(visions.chats, gone),
    };
  }
  return { ...state, visions };
}

/** Older lines the person asked for go before the window, which keeps them from now on. */
function older(state: AppState, action: Extract<VisionsAction, { type: "visions/older" }>): AppState {
  const { channelId, page } = action;
  const window = state.visions.lines[channelId] ?? [];
  const first = window[0]?.seq ?? Infinity;
  const lines = [...page.entries.filter(line => line.seq < first), ...window];
  return {
    ...state,
    visions: {
      ...state.visions,
      lines: { ...state.visions.lines, [channelId]: lines },
      nextBefore: { ...state.visions.nextBefore, [channelId]: page.next_before },
      keep: { ...state.visions.keep, [channelId]: Math.max(lines.length, WINDOW_LINES) },
    },
  };
}

/** Whether `threadId` is a chat a Vision runs (a work chat listed in a channel). */
export function isOwnedChat(state: AppState, threadId: string | null): boolean {
  return threadId !== null && ownedChatIds(state).has(threadId);
}

/** After any action: when the project changed and the open channel is not one of the new
 *  project's, Visions shows that project's `# hq` (the channel was not deleted, it is elsewhere). */
export function visionsFollowProject(before: AppState, after: AppState): AppState {
  const { channelId } = after.visions.open;
  if (after.activeProjectRoot === before.activeProjectRoot || channelId === null) return after;
  const here = (after.visions.channels[after.activeProjectRoot ?? ""] ?? []).some(channel => channel.id === channelId);
  return here ? after : { ...after, visions: { ...after.visions, open: { ...after.visions.open, channelId: null, chatId: null } } };
}

/** The open chat shown again after something else moved the active chat: only the active chat
 *  changes, never where Chats returns to. */
function reshow(state: AppState): AppState {
  const chatId = state.visions.open.chatId;
  return chatId === null ? state : { ...state, activeThreadId: chatId };
}

/** Shows a channel (or `# hq`), and with `chatId` one of its chats, which becomes the active chat;
 *  the plain chat open before it is kept for Chats to return to. */
function open(state: AppState, action: Extract<VisionsAction, { type: "visions/open" }>): AppState {
  const chatId = action.chatId ?? null;
  const plainThreadId = isOwnedChat(state, state.activeThreadId) ? state.visions.open.plainThreadId : state.activeThreadId;
  const visions = { ...state.visions, open: { channelId: action.channelId, chatId, plainThreadId } };
  if (chatId === null) return { ...state, visions };
  const root = Object.keys(visions.channels).find(each => visions.channels[each]!.some(channel => channel.id === action.channelId));
  return { ...state, visions, activeThreadId: chatId, activeProjectRoot: root ?? state.activeProjectRoot };
}

/** Leaving for plain chat: a Vision's chat that was open gives way to the plain chat before it. */
export function leaveOwnedChat(state: AppState): AppState {
  if (!isOwnedChat(state, state.activeThreadId)) return state;
  return { ...state, activeThreadId: state.visions.open.plainThreadId };
}

/** What waits on the person in one channel's own chats, as the server lists it with the channel. */
export function channelNeeds(channel: ChannelInfo): number {
  return channel.needs ?? 0;
}

/** Another project while in Visions: its `# hq`, and its newest plain chat for Chats to return to. */
function project(state: AppState, root: string): AppState {
  const newest = [...(state.threadsByProject[root] ?? [])].sort((a, b) => (b.last_activity_seq ?? 0) - (a.last_activity_seq ?? 0))[0]?.id ?? null;
  return { ...state, activeProjectRoot: root, activeThreadId: newest, visions: { ...state.visions, open: { channelId: null, chatId: null, plainThreadId: newest } } };
}

/** The visions/* actions; null for any other. */
export function reduceVisionsActions(state: AppState, action: AppAction): AppState | null {
  if (action.type === "visions/loaded") return loaded(state, action);
  if (action.type === "visions/older") return older(state, action);
  if (action.type === "visions/open") return open(state, action);
  if (action.type === "visions/project") return project(state, action.root);
  if (action.type === "visions/reshow") return reshow(state);
  if (action.type !== "visions/changed") return null;
  const { channelId, page } = action;
  const v = state.visions;
  const joined = joinNewest(v.lines[channelId] ?? [], v.nextBefore[channelId] ?? null, v.keep[channelId] ?? WINDOW_LINES, page);
  return {
    ...state,
    visions: {
      ...v,
      lines: { ...v.lines, [channelId]: joined.lines },
      nextBefore: { ...v.nextBefore, [channelId]: joined.next },
      chats: { ...v.chats, [channelId]: action.chats },
      questions: { ...v.questions, ...action.questions },
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
  getChannelPage(channelId: string, beforeSeq?: number | null): Promise<ChannelPage>;
  listChannelThreads(channelId: string): Promise<OwnedChat[]>;
  listQuestions(threadId: string): Promise<Array<{ id: string; answer?: string }>>;
}

/** What to read again, merged from the changes that arrived: everything, the Visions list and
 *  channels by id. A Vision named alone (its budget, what waits on the person, its mood) needs
 *  only the list; a write in a channel names that channel. */
interface Wanted {
  all: boolean;
  list: boolean;
  roots: Set<string>;
  channels: Set<string>;
  /** The Visions named: their projects' channel lists are read again (their counts changed). */
  visions: Set<string>;
}

function wanted(): Wanted {
  return { all: false, list: false, roots: new Set(), channels: new Set(), visions: new Set() };
}

function want(into: Wanted, change: VisionsChange, roots: string[] = []): Wanted {
  for (const root of roots) into.roots.add(root);
  if (change.channel_id === undefined && change.vision_id === undefined) into.all = true;
  if (change.vision_id !== undefined) {
    into.list = true;
    into.visions.add(change.vision_id);
  }
  if (change.channel_id !== undefined) into.channels.add(change.channel_id);
  return into;
}

function fulfilled<T>(results: PromiseSettledResult<T>[]): T[] {
  return results.flatMap(result => result.status === "fulfilled" ? [result.value] : []);
}

/** One channel read again: its newest page, its chats and the open questions of the chats its
 *  page passed questions on from. A question read that fails keeps what was read before. */
async function readChannel(bridge: VisionsBridge, channelId: string, shown: Record<string, string[]>): Promise<VisionsAction> {
  const [page, chats] = await Promise.all([bridge.getChannelPage(channelId), bridge.listChannelThreads(channelId)]);
  const passedFrom = new Set(page.entries.flatMap(({ entry }) => entry.type === "passed_on" ? [entry.thread_id] : []));
  const questions: Record<string, string[]> = {};
  await Promise.all([...passedFrom].map(async threadId => {
    try {
      const listed = await bridge.listQuestions(threadId);
      questions[threadId] = listed.filter(question => question.answer === undefined || question.answer === null).map(question => question.id);
    } catch {
      if (shown[threadId] !== undefined) questions[threadId] = shown[threadId];
    }
  }));
  return { type: "visions/changed", channelId, page, chats, questions };
}

/** The projects whose channel lists are read again for `want`. */
function rootsToList(want: Wanted, known: Record<string, ChannelInfo[]>): Set<string> {
  const roots = Object.keys(known);
  const listRoots = new Set<string>(want.all ? [...roots, ...want.roots] : []);
  // A Vision's change (what waits on the person) also changes its channels' counts: only the
  // projects where it is a member are listed again. One pass over the known channels, and at
  // most one listing per project the Vision works in.
  if (want.visions.size > 0) {
    for (const root of roots) {
      if (known[root]!.some(channel => channel.members.some(member => want.visions.has(member)))) listRoots.add(root);
    }
  }
  for (const id of want.channels) {
    const root = roots.find(each => known[each]!.some(channel => channel.id === id));
    // A channel not read yet is new: its project is one of those read.
    for (const each of root !== undefined ? [root] : roots) listRoots.add(each);
  }
  return listRoots;
}

function wantsChannel(want: Wanted, channel: ChannelInfo): boolean {
  return want.all || want.channels.has(channel.id);
}

/** Reads again what `want` names. Each project's channel list and each channel is read on its
 *  own: one that fails leaves what was shown for it, and the rest still land. */
async function reload(bridge: VisionsBridge, want: Wanted, state: AppState, dispatch: (action: VisionsAction) => void): Promise<void> {
  const known = state.visions.channels;
  const listRoots = rootsToList(want, known);
  if (want.all || want.list) {
    try { dispatch({ type: "visions/loaded", visions: await bridge.listVisions() }); } catch { /* the list shown stays */ }
  }
  const listed = await Promise.allSettled([...listRoots].map(async projectRoot => ({ projectRoot, channels: await bridge.listChannels(projectRoot) })));
  const read = new Map<string, ChannelInfo[]>();
  for (const { projectRoot, channels } of fulfilled(listed)) {
    dispatch({ type: "visions/loaded", projectRoot, channels });
    read.set(projectRoot, channels);
  }
  const toRead = [...listRoots].flatMap(root => read.get(root) ?? known[root] ?? []).filter(channel => wantsChannel(want, channel));
  const results = await Promise.allSettled(toRead.map(channel => readChannel(bridge, channel.id, state.visions.questions)));
  for (const action of fulfilled(results)) dispatch(action);
}

/** Reads again what `change` names: a channel (its project's channel list, then its log and
 *  chats), a Vision (the Visions list), or with neither everything for the projects in `roots`
 *  and those already read. */
export function reloadVisions(bridge: VisionsBridge, change: VisionsChange, state: AppState, dispatch: (action: VisionsAction) => void, roots: string[] = []): Promise<void> {
  return reload(bridge, want(wanted(), change, roots), state, dispatch);
}

/** The Vision that drives `threadId`: a work chat's owner, or the Vision whose own chat it is. */
function visionOf(state: AppState, threadId: string): string | undefined {
  const chat = allChats(state).find(owned => owned.id === threadId);
  if (chat) return chat.owner.vision_id;
  return state.visions.list.find(vision => vision.vision_chats.some(own => own.thread_id === threadId))?.id;
}

/** Follows Visions for the app. Changes are read one batch at a time: those arriving while a
 *  read runs are merged into the next one (a later read never lands under an earlier one). An
 *  approval asked or gone in a chat a Vision drives reads the list again (the server announces
 *  its questions and its runs' outcomes itself). Nothing is read or dispatched once
 *  `isCancelled`. */
export function followVisions(bridge: VisionsBridge, getState: () => AppState, dispatch: (action: VisionsAction) => void, isCancelled: () => boolean) {
  let pending: Wanted | null = null;
  let busy = false;
  let done: Promise<void> = Promise.resolve();
  const guarded = (action: VisionsAction) => { if (!isCancelled()) dispatch(action); };
  const drain = async () => {
    try {
      while (pending !== null && !isCancelled()) {
        const next = pending;
        pending = null;
        await reload(bridge, next, getState(), guarded).catch(() => undefined);
      }
    } finally {
      busy = false;
      pending = null;
    }
  };
  const changed = (change: VisionsChange, roots: string[] = []): Promise<void> => {
    pending = want(pending ?? wanted(), change, roots);
    if (!busy) {
      busy = true;
      done = drain();
    }
    return done;
  };
  const approvalChanged = (threadId: string): Promise<void> => {
    const visionId = visionOf(getState(), threadId);
    return visionId === undefined ? Promise.resolve() : changed({ vision_id: visionId });
  };
  /** Reads the page before the channel's window, for scrolling back; whether it added lines. */
  const loadOlder = async (channelId: string): Promise<boolean> => {
    const before = getState().visions.nextBefore[channelId];
    if (before === null || before === undefined || isCancelled()) return false;
    const page = await bridge.getChannelPage(channelId, before);
    const first = getState().visions.lines[channelId]?.[0]?.seq ?? Infinity;
    guarded({ type: "visions/older", channelId, page });
    return !isCancelled() && page.entries.some(line => line.seq < first);
  };
  return { changed, approvalChanged, loadOlder };
}
