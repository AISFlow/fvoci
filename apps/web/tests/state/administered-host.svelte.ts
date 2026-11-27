/** The organization the reader administers, made where its effect can run, for the tests of it. */
import type { Workspace } from '$lib/state/index.ts';
import type { Organizations } from '$lib/state/organization.ts';

import { Administered } from '../../src/app/administered.svelte.ts';

/** `Administered` of a page that is ready, asking `administered` of the server. */
export function administeredBy(administered: () => Promise<string | undefined>) {
  const shown = { status: 'ready' } as unknown as Workspace;
  const api = { administered } as unknown as Organizations;
  let made: Administered | undefined;
  const destroy = $effect.root(() => {
    made = new Administered(
      () => shown,
      () => api,
    );
  });
  if (!made) throw new Error('made in the root');
  return { administered: made, destroy };
}
