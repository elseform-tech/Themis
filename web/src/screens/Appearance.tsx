import { useEffect, useState } from "react";
import { Button } from "../components";
import { SettingsGroup } from "./SettingsGroup";
import type { Settings } from "../lib/types";
import { APPEARANCE_PRESETS, FONT_OPTIONS, appearancePreset, applyAppearance, type Appearance } from "../theme/appearance";

export function AppearanceSettings({ settings, save }: { settings: Settings; save: (patch: Partial<Settings>) => Promise<boolean> }) {
  const [draft, setDraft] = useState<Appearance>(settings);
  useEffect(() => { setDraft(settings); }, [settings]);
  useEffect(() => {
    applyAppearance(draft);
    return () => applyAppearance(settings);
  }, [draft, settings]);
  const changed = draft.theme_palette !== settings.theme_palette || draft.font_family !== settings.font_family || draft.text_size !== settings.text_size;
  async function apply() {
    if (!await save({ theme: "dark", theme_palette: draft.theme_palette, font_family: draft.font_family, text_size: draft.text_size })) setDraft(settings);
  }
  return <SettingsGroup title="Theme">
    <div className="themis-theme-presets">{APPEARANCE_PRESETS.map(preset => <button type="button" key={preset.id} aria-pressed={appearancePreset(draft.theme_palette).id === preset.id} className="themis-theme-preset" onClick={() => setDraft(previous => ({ ...previous, theme_palette: preset.id, font_family: preset.font }))}>
      <span className="themis-theme-swatches" aria-hidden="true">{[preset.colors[0], preset.sidebar, preset.colors[1]].map((color, i) => <span key={i} style={{ background: color }} />)}</span><span>{preset.name}</span>
    </button>)}</div>
    <label className="themis-settings-control">Font<select aria-label="Font" value={draft.font_family} onChange={event => setDraft(previous => ({ ...previous, font_family: event.target.value as Settings["font_family"] }))}>{FONT_OPTIONS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
    <label className="themis-settings-control">Size<span className="themis-text-size"><input aria-label="Size" type="range" min="12" max="22" value={draft.text_size} onChange={event => setDraft(previous => ({ ...previous, text_size: Number(event.target.value) }))} /><output>{draft.text_size} px</output></span></label>
    {changed && <div className="themis-appearance-actions"><span className="themis-settings-hint" role="status">Previewing</span><Button variant="ghost" size="small" onClick={() => setDraft(settings)}>Revert</Button><Button variant="primary" size="small" onClick={() => void apply()}>Apply</Button></div>}
  </SettingsGroup>;
}
