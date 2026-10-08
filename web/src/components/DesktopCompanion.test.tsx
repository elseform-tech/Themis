// @vitest-environment jsdom
import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useDesktopCompanion } from './DesktopCompanion';
import type { KnightMood } from './KnightCompanion';
const show = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ isTauri: () => true }));
vi.mock('@tauri-apps/api/window', () => ({
  Window: class { hide = vi.fn(); show = show; isVisible = async () => false; setSize = vi.fn(); setPosition = vi.fn(); },
  LogicalSize: class {}, PhysicalPosition: class {}, monitorFromPoint: vi.fn(),
  getCurrentWindow: () => ({ isFocused: async () => true, setFocus: vi.fn(), innerPosition: async () => ({ x: 0, y: 0 }), innerSize: async () => ({ width: 800, height: 600 }), scaleFactor: async () => 1 }),
}));
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
it('updates mood without showing the native window again', async () => {
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: vi.fn() });
  const preferences = { enabled: true, size: 96, variant: 'honey' as const, position: { x: 0, y: 0 } };
  function Host({ mood }: { mood: KnightMood }) { useDesktopCompanion(preferences, mood, true, vi.fn()); return null; }
  const view = render(<Host mood="idle" />);
  await act(async () => {});
  expect(show).toHaveBeenCalledTimes(1);
  view.rerender(<Host mood="working" />);
  await act(async () => {});
  expect(show).toHaveBeenCalledTimes(1);
});
