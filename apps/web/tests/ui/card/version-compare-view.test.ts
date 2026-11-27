/**
 * The choice of what a version is compared with (VersionCompare.svelte): it
 * says the version chosen also while the version is shown as it is.
 */
import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { DocNode } from '$lib/editor/schema.ts';
import type { VersionsReading } from '$lib/features/card/body-versions.svelte.ts';
import VersionCompare from '$lib/features/card/VersionCompare.svelte';
import { VersionComparing } from '$lib/features/card/version-compare.svelte.ts';
import type { BodyVersion } from '$lib/state/index.ts';

import { doc, paragraph, text } from '../../support/body-docs.ts';

afterEach(() => {
  localStorage.clear();
  document.body.replaceChildren();
});

const version = (number: number): BodyVersion => ({
  version: number,
  keptAt: number * 1000,
  editors: [],
  isCurrent: false,
  cause: 'auto',
});
const said = (value: string): DocNode => doc(paragraph(text(value)));

describe('the choice of what a version is compared with', () => {
  it('says the version chosen while the version is shown as it is', async () => {
    const versions = [5, 3, 2].map(version);
    const reading = { versions, chosen: versions[0], content: { ...version(5), body: said('b') } };
    const compare = new VersionComparing(reading, {
      read: () => Promise.resolve(said('a')),
      current: () => Promise.resolve(said('c')),
    });
    // Only what the choice reads of a reading.
    const shown: Pick<VersionsReading, 'versions' | 'chosen' | 'content' | 'compare'> = {
      ...reading,
      compare,
    };
    const view = render(VersionCompare, {
      props: {
        reading: shown as VersionsReading,
        whenKept: (one: BodyVersion) => `kept ${String(one.version)}`,
        causeOf: () => 'auto',
        whoWrote: () => '',
        resolveUser: () => '',
        words: { noBody: () => '', invalid: () => '' },
      },
    });
    const choice = view.getByTestId('body-version-compare');
    if (!(choice instanceof HTMLSelectElement)) throw new Error('no choice');
    compare.compareWith(2);
    await vi.waitFor(() => {
      expect(choice.value).toBe('2');
    });
    compare.showWhole(true);
    await vi.waitFor(() => {
      expect(view.getByTestId('body-version-whole').getAttribute('aria-pressed')).toBe('true');
    });
    expect(choice.value).toBe('2');
    view.unmount();
  });
});
