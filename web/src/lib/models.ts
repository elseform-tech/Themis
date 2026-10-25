import type { GoModel } from "./types";

/** Themis's default Go model; mirrors `GO_DEFAULT_MODEL` in crates/core. */
export const DEFAULT_MODEL = "muse-spark-1.3-contributor";

/** A model's option label: a model Themis cannot call says so (the reason is its tooltip). */
export function modelLabel(model: GoModel): string {
  return model.unavailable_reason ? `${model.id} (unavailable)` : model.id;
}

/** Whether a chat on `model` uses OpenCode Go (and so its key), rather than a provider from runtime.jsonc. */
export function usesGo(model: string, models: GoModel[]): boolean {
  return !models.some(entry => entry.id === model && entry.provider);
}

/** `models` grouped by provider, OpenCode Go first, keeping each provider's order. */
export function groupByProvider(models: GoModel[]): Array<{ provider: string | null; models: GoModel[] }> {
  const groups = new Map<string | null, GoModel[]>([[null, []]]);
  for (const model of models) {
    const key = model.provider ?? null;
    groups.set(key, [...(groups.get(key) ?? []), model]);
  }
  return [...groups].filter(([, list]) => list.length > 0).map(([provider, list]) => ({ provider, models: list }));
}
