// Small UI preferences only; credentials stay in the native backend.
export function readSession<T>(key: string, fallback: T): T {
  try { return JSON.parse(localStorage.getItem(`themis:${key}`) ?? 'null') ?? fallback; }
  catch { return fallback; }
}
export function writeSession(key: string, value: unknown): void {
  try { localStorage.setItem(`themis:${key}`, JSON.stringify(value)); } catch { /* Storage may be unavailable. */ }
}
