import { useEffect, useState } from "react";
import { Button } from "../components";
import { SettingsGroup } from "./SettingsGroup";
import type { Settings } from "../lib/types";
import { APPEARANCE_PRESETS, FONT_OPTIONS, appearancePreset, applyAppearance, resolveSky, useAutoSky, type Appearance } from "../theme/appearance";

const SKY_IDS = ["morning", "noon", "night"] as const;

function differs(a: Appearance, b: Appearance): boolean {
  return a.theme_palette !== b.theme_palette || a.font_family !== b.font_family || a.text_size !== b.text_size;
}

function AutoPreset({ selected, onSelect }: { selected: boolean; onSelect: () => void }) {
  return <button type="button" aria-pressed={selected} className="themis-theme-preset" onClick={onSelect}>
    <span className="themis-theme-swatches" aria-hidden="true">{SKY_IDS.map(id => <span key={id} style={{ background: appearancePreset(id).sky }} />)}</span><span>Auto</span>
  </button>;
}

export function AppearanceSettings({ settings, save }: { settings: Settings; save: (patch: Partial<Settings>) => Promise<boolean> }) {
  const [draft, setDraft] = useState<Appearance>(settings);
  useEffect(() => { setDraft(settings); }, [settings]);
  const sky = useAutoSky(draft.theme_palette);
  useEffect(() => {
    applyAppearance({ ...draft, theme_palette: sky });
    return () => applyAppearance({ ...settings, theme_palette: resolveSky(settings.theme_palette) });
  }, [draft, settings, sky]);
  const changed = differs(draft, settings);
  async function apply() {
    if (!await save({ theme: "dark", theme_palette: draft.theme_palette, font_family: draft.font_family, text_size: draft.text_size })) setDraft(settings);
  }
  return <SettingsGroup title="Theme">
    <div className="themis-theme-presets"><AutoPreset selected={draft.theme_palette === "auto"} onSelect={() => setDraft(previous => ({ ...previous, theme_palette: "auto" }))} />{APPEARANCE_PRESETS.map(preset => <button type="button" key={preset.id} aria-pressed={draft.theme_palette !== "auto" && appearancePreset(draft.theme_palette).id === preset.id} className="themis-theme-preset" onClick={() => setDraft(previous => ({ ...previous, theme_palette: preset.id, font_family: preset.font }))}>
      <span className="themis-theme-swatches" aria-hidden="true">{[preset.sky ?? preset.colors[0], preset.sidebar, preset.colors[1]].map((color, i) => <span key={i} style={{ background: color }} />)}</span><span>{preset.name}</span>
    </button>)}</div>
    <label className="themis-settings-control">Font<select aria-label="Font" value={draft.font_family} onChange={event => setDraft(previous => ({ ...previous, font_family: event.target.value as Settings["font_family"] }))}>{FONT_OPTIONS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
    <label className="themis-settings-control">Size<span className="themis-text-size"><input aria-label="Size" type="range" min="10" max="22" value={draft.text_size} onChange={event => setDraft(previous => ({ ...previous, text_size: Number(event.target.value) }))} /><output>{draft.text_size} px</output></span></label>
    {changed && <div className="themis-appearance-actions"><span className="themis-settings-hint" role="status">Previewing</span><Button variant="ghost" size="small" onClick={() => setDraft(settings)}>Revert</Button><Button variant="primary" size="small" onClick={() => void apply()}>Apply</Button></div>}
  </SettingsGroup>;
}
