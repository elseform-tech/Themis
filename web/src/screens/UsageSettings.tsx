import { useCallback, useEffect, useState } from "react";
import { getUsageSummary } from "../lib/tauri";
import type { UsageBreakdown, UsageSummary, UsageTotals } from "../lib/types";
import { describeError } from "../state/store";
import { RefreshIcon, SettingsGroup, SettingsIconButton } from "./SettingsGroup";
import "./UsageSettings.css";

/** Categorical slots for the dark surface, in validated order; the rest fold into Other. */
const SERIES = ["#3987e5", "#d95926", "#199e70", "#c98500"];
const OTHER = { name: "Other", color: "#6f7480" };

/** 950, 12.3k, 4.56M. */
export function compactTokens(count: number): string {
  if (count < 1000) return String(count);
  if (count < 1_000_000) return `${(count / 1000).toFixed(1)}k`;
  return `${(count / 1_000_000).toFixed(2)}M`;
}

/** "≈$0.42", with "+" when some requests had no published price; null when nothing was priced. */
export function costLabel(totals: UsageTotals): string | null {
  if (totals.estimated_cost_usd === undefined) return null;
  const cost = totals.estimated_cost_usd;
  const text = cost > 0 && cost < 0.01 ? "<$0.01" : `$${cost.toFixed(2)}`;
  return `≈${text}${totals.unpriced_requests > 0 ? "+" : ""}`;
}

const WINDOWS: Array<[keyof UsageSummary["windows"], string]> = [["five_hours", "5 hours"], ["week", "This week"], ["month", "This month"]];

function dayLabel(day: string) {
  const [year, month, date] = day.split("-").map(Number);
  return new Date(year, month - 1, date).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function WindowsTable({ summary }: { summary: UsageSummary }) {
  const priced = WINDOWS.some(([key]) => summary.windows[key].estimated_cost_usd !== undefined);
  return <table className="themis-usage-windows">
    <thead><tr>
      <th scope="col"><span className="themis-visually-hidden">Window</span></th>
      <th scope="col" title="Includes cached input">Input</th>
      <th scope="col" title="Includes reasoning">Output</th>
      <th scope="col">Requests</th>
      {priced && <th scope="col" title="Estimated at public list prices (models.dev). Your plan bills differently; Themis does not know your allowance.">≈ List price</th>}
    </tr></thead>
    <tbody>{WINDOWS.map(([key, label]) => {
      const totals = summary.windows[key];
      const cached = totals.input_tokens > 0 ? Math.round(totals.cached_input_tokens / totals.input_tokens * 100) : 0;
      const unpriced = totals.unpriced_requests > 0 ? `${totals.unpriced_requests} request${totals.unpriced_requests === 1 ? "" : "s"} without a published price` : undefined;
      return <tr key={key}>
        <th scope="row">{label}</th>
        <td>{compactTokens(totals.input_tokens)}{totals.cached_input_tokens > 0 && <span className="themis-usage-sub"> · {cached}% cached</span>}</td>
        <td>{compactTokens(totals.output_tokens)}{totals.reasoning_tokens > 0 && <span className="themis-usage-sub"> · {compactTokens(totals.reasoning_tokens)} reasoning</span>}</td>
        <td title={totals.unreported_requests > 0 ? `${totals.unreported_requests} without token counts from the provider` : undefined}>{totals.requests}</td>
        {priced && <td title={unpriced}>{costLabel(totals) ?? "—"}</td>}
      </tr>;
    })}</tbody>
  </table>;
}

type Series = { name: string; color: string; total: number; free: boolean; other?: boolean };

/** Up to four models by 30-day tokens get their own color; with more, the fourth slot sums the rest as Other. */
export function seriesOf(models: UsageBreakdown[]): Series[] {
  const named = models.slice(0, models.length > SERIES.length ? SERIES.length - 1 : SERIES.length);
  const rest = models.slice(named.length);
  const size = (totals: UsageTotals) => totals.input_tokens + totals.output_tokens;
  const series: Series[] = named.map((model, index) => ({ name: model.key ?? "unknown", color: SERIES[index], total: size(model.totals), free: !!model.free }));
  if (rest.length > 0) series.push({ ...OTHER, total: rest.reduce((sum, model) => sum + size(model.totals), 0), free: false, other: true });
  return series;
}

function DailyChart({ summary }: { summary: UsageSummary }) {
  const [active, setActive] = useState<number | null>(null);
  const series = seriesOf(summary.by_model);
  const days = summary.days.map(day => {
    const parts = series.map(item => ({ ...item, tokens: 0 }));
    for (const [model, tokens] of Object.entries(day.models)) {
      const part = parts.find(entry => !entry.other && entry.name === model) ?? parts.find(entry => entry.other);
      if (part) part.tokens += tokens;
    }
    return { day: day.day, requests: day.requests, parts, total: parts.reduce((sum, part) => sum + part.tokens, 0) };
  });
  const max = Math.max(1, ...days.map(day => day.total));
  const shown = active === null ? null : days[active];
  return <>
    <div className="themis-usage-chart" onMouseLeave={() => setActive(null)}>
      <span className="themis-usage-max" aria-hidden="true">{compactTokens(max)}</span>
      <div className="themis-usage-bars">
        {days.map((day, index) => <div key={day.day} className={`themis-usage-day${active === index ? " is-active" : ""}`} tabIndex={0}
          role="img" aria-label={`${dayLabel(day.day)}: ${compactTokens(day.total)} tokens${day.parts.filter(part => part.tokens > 0).map(part => `, ${part.name} ${compactTokens(part.tokens)}`).join("")}`}
          onMouseEnter={() => setActive(index)} onFocus={() => setActive(index)} onBlur={() => setActive(null)}>
          <div className="themis-usage-stack" style={{ height: `${day.total / max * 100}%` }}>
            {day.parts.filter(part => part.tokens > 0).map(part => <span key={part.name} style={{ flexGrow: part.tokens, background: part.color }} />)}
          </div>
        </div>)}
      </div>
      {shown && <div className="themis-usage-tip" role="status" style={{ left: `${Math.min(80, Math.max(20, (active! + 0.5) / days.length * 100))}%` }}>
        <strong>{dayLabel(shown.day)}</strong>
        <span>{compactTokens(shown.total)} tokens · {shown.requests} request{shown.requests === 1 ? "" : "s"}</span>
        {shown.parts.filter(part => part.tokens > 0).map(part => <span key={part.name} className="themis-usage-tip-row"><i style={{ background: part.color }} />{part.name}<b>{compactTokens(part.tokens)}</b></span>)}
      </div>}
      <div className="themis-usage-axis" aria-hidden="true">
        <span>{dayLabel(days[0].day)}</span><span>{dayLabel(days[days.length - 1].day)}</span>
      </div>
    </div>
    <ul className="themis-usage-legend" aria-label="Models">
      {series.map(item => <li key={item.name}><span style={{ background: item.color }} aria-hidden="true" />{item.name}{item.free && <em>Free</em>}<b>{compactTokens(item.total)}</b></li>)}
    </ul>
  </>;
}

/** Settings › Usage: tokens Themis used, the last 30 days by model, and the chats that used most. */
export function UsageSettingsSection() {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const load = useCallback(async () => {
    setLoading(true);
    try { setSummary(await getUsageSummary()); setError(""); } catch (reason) { setError(describeError(reason)); } finally { setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);
  if (!summary) return error ? <p role="alert" className="themis-form-error">{error}</p> : null;
  const any = summary.days.some(day => day.requests > 0);
  const chats = summary.by_chat.filter(chat => chat.totals.requests > 0);
  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    {!summary.available && <p role="alert" className="themis-form-error">Usage records could not be opened; new requests are not being counted.</p>}
    <SettingsGroup title="Tokens · from Themis only" hint="Requests this Themis made, as each provider reported them. Other apps using the same account are not counted, so the provider's own figures may be higher."
      action={<SettingsIconButton label="Refresh usage" disabled={loading} onClick={() => void load()}><RefreshIcon /></SettingsIconButton>}>
      <WindowsTable summary={summary} />
    </SettingsGroup>
    {any && <SettingsGroup title="Last 30 days" hint="Input and output tokens per day, by model. Days follow this computer's time zone; weeks start on Monday.">
      <div className="themis-usage-chart-card"><DailyChart summary={summary} /></div>
    </SettingsGroup>}
    {chats.length > 0 && <SettingsGroup title="Top chats · 30 days">
      {chats.map(chat => <div key={chat.key ?? "deleted"} className="themis-settings-control themis-usage-chat">
        <span className={chat.key ? undefined : "themis-usage-sub"} title={chat.title}>{chat.title ?? "Deleted chats"}</span>
        <span className="themis-usage-sub">{compactTokens(chat.totals.input_tokens + chat.totals.output_tokens)} tokens · {chat.totals.requests} request{chat.totals.requests === 1 ? "" : "s"}{costLabel(chat.totals) ? ` · ${costLabel(chat.totals)}` : ""}</span>
      </div>)}
    </SettingsGroup>}
  </>;
}
