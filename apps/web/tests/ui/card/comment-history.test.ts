/**
 * What a comment said before (CommentHistory.svelte): each text after the
 * first says what it changed from the one before it, words added and
 * removed, the text the comment has now last; the first is as it was; a
 * text the same as the one before says so; and one that cannot be shown is
 * shown as it is, not compared.
 */
import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { BodyReferences } from '$lib/editor/body-editor.ts';
import type { DocNode } from '$lib/editor/schema.ts';
import CommentHistory from '$lib/features/card/CommentHistory.svelte';
import type { CommentVersion } from '$lib/state/index.ts';

import { doc, paragraph, text } from '../../support/body-docs.ts';

afterEach(() => {
  document.body.replaceChildren();
});

const said = (value: string): DocNode => doc(paragraph(text(value)));
const version = (number: number, body: DocNode | null): CommentVersion => ({
  version: number,
  body,
  writtenBy: 'u1',
  writtenAt: number * 60_000,
});

function history(earlier: CommentVersion[], now: DocNode | null): ReturnType<typeof render> {
  return render(CommentHistory, {
    props: {
      read: () => Promise.resolve(earlier),
      references: {} as BodyReferences,
      commentId: 'c1',
      editedAt: 10 * 60_000,
      body: now,
      editedBy: 'u2',
      context: { locale: 'en', timeZone: 'UTC', hourCycle: 'h23' },
      resolveUser: (userId: string) => (userId === 'u1' ? 'Ann' : 'Bo'),
      id: 'history',
    },
  });
}

/** The texts listed, each as what is marked in it: `+added`, `-removed`. */
function marks(view: ReturnType<typeof render>): string[][] {
  return [...view.container.querySelectorAll('[data-version]')].map((one) =>
    [...one.querySelectorAll('ins, del')].map(
      (mark) => (mark.tagName === 'INS' ? '+' : '-') + mark.textContent,
    ),
  );
}

describe('the texts of a comment', () => {
  it('say what each changed from the one before, the first as it was, the text now last', async () => {
    const view = history(
      [version(1, said('Buy milk today')), version(2, said('Buy oat milk today'))],
      said('Buy oat milk tomorrow'),
    );
    await vi.waitFor(() => {
      expect(marks(view)).toEqual([[], ['+oat '], ['-today', '+tomorrow']]);
    });
    const texts = [...view.container.querySelectorAll('[data-version]')];
    expect(texts.map((one) => one.getAttribute('data-version'))).toEqual(['1', '2', 'now']);
    expect(texts[0]?.textContent).toContain('Buy milk today');
    expect(texts[2]?.textContent).toContain('Now, changed by Bo');
    expect(texts[2]?.querySelector('[data-testid="comment-diff-counts"]')?.textContent).toBe(
      '1 added · 1 removed',
    );
    // Heard too: what each mark is.
    expect(texts[2]?.querySelector('ins')?.getAttribute('data-diff-label')).toBe('Added');
  });

  it('says a text is the same as the one before it, and shows one that cannot be shown as it is', async () => {
    const broken = { type: 'doc', content: [{ type: 'nothing-known' }] } as unknown as DocNode;
    const view = history([version(1, said('Same')), version(2, said('Same'))], broken);
    await vi.waitFor(() => {
      expect(view.container.querySelectorAll('[data-testid="comment-diff-counts"]')).toHaveLength(
        1,
      );
    });
    const [, second, now] = [...view.container.querySelectorAll('[data-version]')];
    expect(second?.querySelector('[data-testid="comment-diff-counts"]')?.textContent).toBe(
      'Nothing has changed.',
    );
    expect(now?.querySelectorAll('ins, del')).toHaveLength(0);
    expect(now?.textContent).toContain('This description cannot be shown');
  });
});
