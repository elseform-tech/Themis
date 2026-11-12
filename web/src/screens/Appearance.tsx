import { useEffect, useState } from "react";
import { Button } from "../components";
import { SettingsGroup } from "./SettingsGroup";
import type { Settings } from "../lib/types";
import { AUTO_PREVIEW, FONT_OPTIONS, appearancePreset, applyAppearance, resolveSky, useAutoSky, type Appearance } from "../theme/appearance";

const SKY_OPTIONS = [
  { id: "auto", name: "Auto", preview: AUTO_PREVIEW },
  ...["morning", "noon", "night", "classic"].map(id => appearancePreset(id)),
];

function differs(a: Appearance, b: Appearance): boolean {
  return a.theme_palette !== b.theme_palette || a.font_family !== b.font_family || a.text_size !== b.text_size;
}

/** The chosen card: Auto, or the preset an id shows (an unknown or removed one shows Classic). */
function chosen(palette: string): string {
  return palette === "auto" ? "auto" : appearancePreset(palette).id;
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
  const selected = chosen(draft.theme_palette);
  async function apply() {
    if (!await save({ theme_palette: draft.theme_palette, font_family: draft.font_family, text_size: draft.text_size })) setDraft(settings);
  }
  return <>
    <div className="themis-theme-presets" role="group" aria-label="Sky">{SKY_OPTIONS.map(option => <button type="button" key={option.id} aria-pressed={selected === option.id} className="themis-theme-preset" onClick={() => setDraft(previous => ({ ...previous, theme_palette: option.id }))}>
      <span className="themis-theme-preview" aria-hidden="true" style={{ background: option.preview }} /><span>{option.name}</span>
    </button>)}</div>
    <SettingsGroup>
      <label className="themis-settings-control">Font<select aria-label="Font" value={draft.font_family} onChange={event => setDraft(previous => ({ ...previous, font_family: event.target.value as Settings["font_family"] }))}>{FONT_OPTIONS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
      <label className="themis-settings-control">Size<span className="themis-text-size"><input aria-label="Size" type="range" min="10" max="22" value={draft.text_size} onChange={event => setDraft(previous => ({ ...previous, text_size: Number(event.target.value) }))} /><output>{draft.text_size} px</output></span></label>
      {changed && <div className="themis-appearance-actions"><span className="themis-settings-hint" role="status">Previewing</span><Button variant="ghost" size="small" onClick={() => setDraft(settings)}>Revert</Button><Button variant="primary" size="small" onClick={() => void apply()}>Apply</Button></div>}
    </SettingsGroup>
  </>;
}
