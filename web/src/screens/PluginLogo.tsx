import { useEffect, useState } from "react";
import brandLogos from "../lib/brandLogos.json";
import { pluginAction } from "../lib/tauri";
import type { PluginSpec } from "../lib/types";

const logos: Record<string, { hex: string; path: string }> = brandLogos;
/** Saved icons are read once per name; they never change, because the name is their hash. */
const assets = new Map<string, Promise<string>>();
function loadAsset(asset: string) {
  let pending = assets.get(asset);
  if (!pending) {
    pending = pluginAction<string>({ action: "plugin_icon", asset });
    assets.set(asset, pending);
  }
  return pending;
}
/** A steady hue per name, so a plugin's letter tile keeps its colour. */
const tileHue = (name: string) => [...name].reduce((hash, char) => (hash * 31 + char.charCodeAt(0)) % 360, 7);
const luminance = (hex: string) => {
  const [r, g, b] = [0, 2, 4].map(start => parseInt(hex.slice(start, start + 2), 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
/** Plugin icon, in order: its own saved image, a bundled brand logo, then a tile with its
 *  initial on a colour from its name. Nothing here reaches the network. */
export function PluginLogo({ spec, brand, name, small }: { spec?: PluginSpec; brand?: string; name: string; small?: boolean }) {
  const asset = spec?.icon_asset;
  const bundledSvg = spec?.icon && spec.files?.[spec.icon]?.trim().startsWith("<svg") ? spec.files[spec.icon] : undefined;
  const [loaded, setLoaded] = useState<{ asset: string; url: string } | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    if (!asset) return;
    let cancelled = false;
    void loadAsset(asset).then(url => { if (!cancelled) setLoaded({ asset, url }); }, () => { if (!cancelled) setFailed(asset); });
    return () => { cancelled = true; };
  }, [asset]);
  const className = `themis-card-tile${small ? " themis-card-tile-small" : ""}`;
  const image = asset && failed !== asset ? (loaded?.asset === asset ? loaded.url : undefined) : bundledSvg ? `data:image/svg+xml,${encodeURIComponent(bundledSvg)}` : undefined;
  if (image) return <span className={className}><img src={image} alt="" onError={() => setFailed(asset ?? null)} /></span>;
  const logo = logos[spec?.brand ?? brand ?? ""];
  if (logo) {
    // A very dark mark sits on the neutral tile, so it doesn't disappear into the dark theme.
    const dark = luminance(logo.hex) < 0.04;
    return <span className={`${className} themis-card-brand`} style={dark ? undefined : { background: `#${logo.hex}`, color: luminance(logo.hex) > 0.55 ? "#131314" : "#ffffff" }} aria-hidden="true">
      <svg viewBox="0 0 24 24" fill="currentColor"><path d={logo.path} /></svg>
    </span>;
  }
  return <span className={`${className} themis-card-monogram`} style={{ ["--tile-hue" as string]: tileHue(name) }} aria-hidden="true">{name.charAt(0).toUpperCase()}</span>;
}
