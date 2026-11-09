// Settings fields stay live while other clients write settings, without clobbering an edit in
// progress: a field follows the saved value until the person changes it, and keeps their draft
// (saved on blur, as before) once they have.
import { useState, type Dispatch, type SetStateAction } from "react";

/** `form` with each of `drafts` kept where the person changed it from `shown` (the saved
 *  values the form last showed); every other field takes `next`. */
export function keepDrafts<T extends object>(form: T, shown: T, next: T, drafts: readonly (keyof T)[]): T {
  const merged = { ...next };
  for (const key of drafts) if (!Object.is(form[key], shown[key])) merged[key] = form[key];
  return merged;
}

/** A form over `saved` that follows it when it changes elsewhere, except the `drafts` fields
 *  the person is editing. */
export function useFormDraft<T extends object>(saved: T, drafts: readonly (keyof T)[]): [T, Dispatch<SetStateAction<T>>] {
  const [form, setForm] = useState(saved);
  const [shown, setShown] = useState(saved);
  if (shown !== saved) {
    setShown(saved);
    setForm(current => keepDrafts(current, shown, saved, drafts));
  }
  return [form, setForm];
}

/** One field's draft: follows `saved` while it still shows the last saved value. */
export function useDraft<T>(saved: T): [T, Dispatch<SetStateAction<T>>] {
  const [draft, setDraft] = useState(saved);
  const [shown, setShown] = useState(saved);
  if (!Object.is(shown, saved)) {
    setShown(saved);
    if (Object.is(draft, shown)) setDraft(saved);
  }
  return [draft, setDraft];
}
