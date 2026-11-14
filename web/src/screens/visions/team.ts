// A channel's team as the screens show it (boards TeamChannel, ChiefProfile): its chief and
// workers, the chief's expression and each worker's status. Also the shapes of team data the
// server does not send yet (a worker's thread with its chief, routines, requests between channels,
// team and hire proposals): the screens are built against them and get them through props.
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import type { useApp } from "../../state/store";
import { expressionFor, EXPRESSION_DOT, EXPRESSION_LABEL, type Expression } from "./Expression";

type AppState = ReturnType<typeof useApp>["state"];

export interface Team {
  chief: VisionInfo | undefined;
  workers: VisionInfo[];
}

/** The channel's chief and its workers, as listed now (members not listed are left out). */
export function teamOf(state: AppState, channel: ChannelInfo): Team {
  const byId = (id: string) => state.visions.list.find(vision => vision.id === id);
  return {
    chief: byId(channel.chief),
    workers: channel.members.filter(id => id !== channel.chief).flatMap(id => byId(id) ?? []),
  };
}

/** Whether the Vision leads any channel (Atlas leads `# administration`). */
export function leadsAChannel(state: AppState, visionId: string): boolean {
  return Object.values(state.visions.channels).some(channels => channels.some(channel => channel.chief === visionId));
}

/** Whether any chat the Vision runs in this channel (its own, or a work chat) is going now. */
function runsHere(state: AppState, channel: ChannelInfo, vision: VisionInfo, own: boolean): boolean {
  const going = (id: string, listed: boolean) => state.running[id] ?? listed;
  if (own && vision.vision_chats.some(chat => chat.channel_id === channel.id && going(chat.thread_id, chat.running))) return true;
  return (state.visions.chats[channel.id] ?? []).some(chat => chat.owner.vision_id === vision.id && going(chat.id, chat.running));
}

/** The chief's face in its channel's header and profile. */
export function chiefExpression(state: AppState, channel: ChannelInfo, chief: VisionInfo): Expression {
  return expressionFor({
    running: runsHere(state, channel, chief, true),
    waitingOnPerson: (channel.needs ?? 0) > 0 || chief.needs > 0,
    finished: false,
    failed: chief.sorry,
    blocked: false,
    paused: chief.paused,
    overBudget: chief.over_budget,
  });
}

export interface Status { label: string; dot: string }

export const statusOf = (expression: Expression): Status => ({ label: EXPRESSION_LABEL[expression], dot: EXPRESSION_DOT[expression] });

/** A worker's status dot on its chip and in the chief's Team list: Idle when nothing applies. */
export function workerStatus(state: AppState, channel: ChannelInfo, worker: VisionInfo): Status {
  const expression = expressionFor({
    running: runsHere(state, channel, worker, true),
    waitingOnPerson: worker.needs > 0,
    finished: false,
    failed: worker.sorry,
    blocked: false,
    paused: worker.paused || (channel.paused_workers ?? []).includes(worker.id),
    overBudget: worker.over_budget,
  });
  return expression === "calm" ? { label: "Idle", dot: EXPRESSION_DOT.calm } : statusOf(expression);
}

// ---- Not sent by the server yet: the screens take these through props. ----

/** One line of a worker's thread with its chief (needs a person-facing read of `get_team_log`).
 *  Tasks, redirects, pauses and resumes are the chief's; reports and pushback the worker's. */
export type TeamEntry =
  | { kind: "task"; text: string }
  | { kind: "redirect"; text: string }
  | { kind: "report"; text: string }
  | { kind: "pushback"; text: string }
  | { kind: "paused" }
  | { kind: "resumed" };

export interface TeamLine { seq: number; at_ms: number; entry: TeamEntry }

/** When a routine fires (Task 7's `Schedule`): days of the week, 0 = Sunday, at local "HH:MM". */
export interface Schedule { days: number[]; time: string }

/** A chief's routine (Task 7's `Routine`). */
export interface Routine {
  id: string;
  chief: string;
  channel_id: string;
  schedule: Schedule;
  instruction: string;
  enabled: boolean;
}

/** A request between channels or a chief's new routine, as one line in the channel (Tasks 5, 7).
 *  The request lines carry the server's fields (`vision_id`, `request_id`, `channel_id`, `ask`);
 *  `answer` stands in for the server's separate `request_answered` line until it is wired. */
export type TeamNoteEntry =
  | { type: "request_sent"; vision_id: string; request_id: string; channel_id: string; ask: string; answer?: "accepted" | "declined" }
  | { type: "request_received"; vision_id: string; request_id: string; channel_id: string; ask: string; answer?: "accepted" | "declined" }
  | { type: "routine_added"; vision_id: string; routine_id: string; schedule: Schedule; instruction: string };

export const TEAM_NOTE_TYPES: ReadonlySet<string> = new Set<TeamNoteEntry["type"]>(["request_sent", "request_received", "routine_added"]);

const isText = (value: unknown): value is string => typeof value === "string";

/** The line as a team note, or null when a field it shows is missing or of the wrong kind. */
export function teamNote(entry: unknown): TeamNoteEntry | null {
  const line = entry as Record<string, unknown> | null;
  if (!line || !TEAM_NOTE_TYPES.has(line.type as string)) return null;
  if (line.type === "routine_added") {
    const schedule = line.schedule as Partial<Schedule> | undefined;
    const ok = isText(line.instruction) && Array.isArray(schedule?.days) && schedule.days.every(day => typeof day === "number") && isText(schedule.time);
    return ok ? line as unknown as TeamNoteEntry : null;
  }
  return isText(line.channel_id) && isText(line.ask) ? line as unknown as TeamNoteEntry : null;
}

/** A team proposal or a hire, as Task 6 will send it inside a proposal. */
export interface ProposedMember { name: string; role?: string; shape?: string; color?: string; accessory?: string; model?: string }
export interface TeamProposal {
  team?: { channel: { name: string; topic?: string }; chief: ProposedMember; workers: ProposedMember[]; autonomy?: ChannelInfo["autonomy"]; daily_budget_usd?: number };
  hire?: { channel_id: string; worker: ProposedMember };
}

const DAY = ["Sundays", "Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays"];
const SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** "Weekdays 09:00", "Fridays 15:00", "Every day 08:00", "Mon, Thu 10:00". */
export function scheduleWords(schedule: Schedule): string {
  const days = [...new Set(schedule.days)].filter(day => day >= 0 && day <= 6).sort();
  const key = days.join("");
  let when: string;
  if (key === "0123456") when = "Every day";
  else if (key === "12345") when = "Weekdays";
  else if (key === "06") when = "Weekends";
  else if (days.length === 1) when = DAY[days[0]!]!;
  else when = days.map(day => SHORT[day]).join(", ");
  return `${when} ${schedule.time}`;
}
