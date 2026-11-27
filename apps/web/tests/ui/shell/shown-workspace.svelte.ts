/**
 * A workspace as the stack of transfers reads it (Transfers.svelte): its
 * transfers, and its info, which the page replaces, with the same id too.
 */
import type { Workspace } from '$lib/state/index.ts';
import type { Transfers } from '$lib/state/transfers.svelte.ts';

export class ShownWorkspace {
  info = $state.raw<{ id: string } | undefined>();
  readonly transfers: Transfers;

  constructor(transfers: Transfers, id: string) {
    this.transfers = transfers;
    this.info = { id };
  }

  get workspace(): Workspace {
    return this as unknown as Workspace;
  }
}
