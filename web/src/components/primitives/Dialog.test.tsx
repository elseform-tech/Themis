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
