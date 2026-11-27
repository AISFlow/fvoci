/**
 * The line under a card's body that says what came of the keys that keep a
 * version (CardBody.svelte): nothing is left running once the card is closed
 * while the version is being kept.
 */
import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { KeepOutcome } from '$lib/features/card/body-versions.svelte.ts';
import CardBody from '$lib/features/card/CardBody.svelte';
import { BoardStore, CardStore, Workspace } from '$lib/state/index.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import { seedWorld } from '$lib/state/memory/seed.ts';

import { doc, paragraph, text } from '../../support/body-docs.ts';

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
});

/** A card's body with no room (none is opened: the workspace is not known), kept by `keep`. */
function body(keep: () => Promise<KeepOutcome>): ReturnType<typeof render> {
  const { world, readerId } = seedWorld([]);
  const workspace = new Workspace({
    backend: new MemoryBackend(world, readerId),
    locale: () => 'en',
  });
  workspace.me = {
    id: 'u1',
    username: 'ada',
    displayName: 'Ada',
    email: '',
    createdAt: 0,
    updatedAt: 0,
  };
  const container = new BoardStore(workspace, 'b1');
  container.members = [{ containerId: 'b1', userId: 'u1', role: 'editor', synthetic: false }];
  const card = new CardStore(container, 'c1', () => undefined);
  card.body = doc(paragraph(text('The body')));
  return render(CardBody, { props: { container, card, keep } });
}

const pressSave = (on: Element): void => {
  on.dispatchEvent(
    new KeyboardEvent('keydown', { key: 's', code: 'KeyS', ctrlKey: true, bubbles: true }),
  );
};

describe('what came of the keys', () => {
  it('leaves no timer behind when the card is closed while the version is kept', async () => {
    vi.useFakeTimers();
    let answer: (outcome: KeepOutcome) => void = () => undefined;
    const keep = vi.fn(
      () =>
        new Promise<KeepOutcome>((resolve) => {
          answer = resolve;
        }),
    );
    const view = body(keep);
    const paragraphOf = view.container.querySelector('p');
    if (!paragraphOf) throw new Error('no body drawn');
    pressSave(paragraphOf);
    expect(keep).toHaveBeenCalledTimes(1);
    view.unmount();
    answer('kept');
    await vi.advanceTimersByTimeAsync(0);
    expect(vi.getTimerCount()).toBe(0);
  });
});
