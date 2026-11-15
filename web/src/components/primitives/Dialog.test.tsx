// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Dialog } from './Dialog';
afterEach(cleanup);
it('includes editable prompts in autofocus and keyboard wrapping', () => {
  render(<Dialog open title="Editor" onClose={vi.fn()}><div contentEditable role="textbox" aria-label="Instructions" /><button>Save</button></Dialog>);
  const prompt = screen.getByRole('textbox');
  expect(prompt).toHaveFocus();
  fireEvent.keyDown(prompt, { key: 'Tab', shiftKey: true });
  expect(screen.getByRole('button')).toHaveFocus();
  fireEvent.keyDown(document.activeElement!, { key: 'Tab' });
  expect(prompt).toHaveFocus();
});
it('opens above the whole window, outside the panel that renders it', () => {
  // A glass panel (backdrop-filter) would trap a fixed overlay rendered inside it.
  render(<div data-testid="panel"><Dialog open title="New channel" onClose={vi.fn()}><button>Create</button></Dialog></div>);
  const dialog = screen.getByRole('dialog');
  expect(screen.getByTestId('panel')).not.toContainElement(dialog);
  expect(dialog.closest('.themis-dialog-overlay')?.parentElement).toBe(document.body);
});
