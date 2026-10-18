import type { GoModel } from "./types";

/** Themis's default Go model; mirrors `GO_DEFAULT_MODEL` in crates/core. */
export const DEFAULT_MODEL = "muse-spark-1.3-contributor";

/** A model's option label: a model Themis cannot call says so (the reason is its tooltip). */
export function modelLabel(model: GoModel): string {
  return model.unavailable_reason ? `${model.id} (unavailable)` : model.id;
}
