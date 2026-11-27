/**
 * What the editor asks before a save that is not a plain new version
 * (SaveChoice.svelte): its title is what weighs most, and its description,
 * which a screen reader reads with it, says all that is asked, in order:
 * the protection taken away, the losses, the counts that differ.
 */
import { cleanup, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';

import SaveChoice from '$lib/components/file-viewer/SaveChoice.svelte';
import type { EditingSession } from '$lib/file-viewer/editing.svelte.ts';
import type { Loss, Recount } from '$lib/file-viewer/editor-protocol.ts';
import { m } from '$lib/i18n/paraglide/messages.js';

const LOSS: Loss = {
  code: 'partOmitted',
  subject: 'stream',
  reason: 'notKeptByEditor',
  path: 'ViewText/Section0',
};
const PAGES: Recount = { of: 'pages', before: 157, after: 159 };

/** A session asking about `losses` and `recounts`, the protection taken away or not. */
function asking(unprotected: boolean, losses: Loss[], recounts: Recount[]): EditingSession {
  const exported = { request: 1, file: new File([], '문서.hwp') };
  const saving = { name: 'lossy', count: losses.length, losses, unprotected, recounts, exported };
  const nothing = (): void => undefined;
  return { saving, dismiss: nothing, saveApart: nothing, saveWithLosses: nothing } as never;
}

/** The dialog's title and the description it is described by. */
function said(): { title: string; description: string } {
  const dialog = screen.getByRole('alertdialog');
  const named = (attribute: string): string =>
    document.getElementById(dialog.getAttribute(attribute) ?? '')?.textContent ?? '';
  return { title: named('aria-labelledby'), description: named('aria-describedby') };
}

afterEach(() => {
  cleanup();
});

describe('the question before a save', () => {
  it('says the counts with their own description where nothing else is asked', () => {
    render(SaveChoice, { session: asking(false, [], [PAGES]) });
    expect(said()).toEqual({
      title: m.file_editor_recount_title(),
      description: m.file_editor_recount_description(),
    });
    expect(screen.getByRole('region', { name: m.file_editor_changes_list() })).toHaveProperty(
      'tabIndex',
      0,
    );
  });

  it('says the counts after the losses as listed after them', () => {
    render(SaveChoice, { session: asking(false, [LOSS], [PAGES]) });
    expect(said()).toEqual({
      title: m.file_editor_loss_title(),
      description: [
        m.file_editor_loss_description({ count: 1 }),
        m.file_editor_recount_also(),
      ].join(' '),
    });
  });

  it('says the counts with their own description after the protection, nothing lost', () => {
    render(SaveChoice, { session: asking(true, [], [PAGES]) });
    expect(said()).toEqual({
      title: m.file_editor_unprotect_title(),
      description: [
        m.file_editor_unprotect_description(),
        m.file_editor_recount_description(),
      ].join(' '),
    });
  });

  it('says the protection, the losses and the counts, in that order', () => {
    render(SaveChoice, { session: asking(true, [LOSS], [PAGES]) });
    expect(said().description).toBe(
      [
        m.file_editor_unprotect_description(),
        m.file_editor_unprotect_losses({ count: 1 }),
        m.file_editor_recount_also(),
      ].join(' '),
    );
  });

  it('says only the protection where nothing else is asked, and lists nothing', () => {
    render(SaveChoice, { session: asking(true, [], []) });
    expect(said().description).toBe(m.file_editor_unprotect_description());
    expect(screen.queryByRole('region')).toBeNull();
  });
});
