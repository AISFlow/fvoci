/**
 * What the reader watches, on the server (`createSubscription`,
 * `deleteSubscription`, `getSubscriptions`).
 */
import type { WatchedKind, Watches } from '../state/watch-types.ts';

import type { Schemas } from './convert.ts';
import type { RequestOptions } from './http.ts';
import type { WorkspacePath } from './requests.ts';

/** A request under the workspace `workspaceId`. */
type Ask = <T>(
  path: WorkspacePath,
  params: Record<string, string>,
  options: RequestOptions,
  workspaceId: string,
) => Promise<T>;

export class ServerWatches implements Watches {
  readonly #ask: Ask;

  constructor(ask: Ask) {
    this.#ask = ask;
  }

  async watching(workspaceId: string, userId: string): Promise<string[]> {
    const subs = await this.#ask<Schemas['Subscription'][]>(
      '/subscriptions/{userId}',
      { userId },
      {},
      workspaceId,
    );
    return subs.filter((sub) => sub.deletedAt === undefined).map((sub) => sub.blockId);
  }

  async watch(
    workspaceId: string,
    blockId: string,
    kind: WatchedKind,
    userId: string,
  ): Promise<void> {
    const json: Schemas['NewSubscription'] = { blockId, blockType: kind, userId };
    await this.#ask<Schemas['Subscription']>(
      '/subscriptions',
      {},
      { method: 'POST', json },
      workspaceId,
    );
  }

  async unwatch(workspaceId: string, blockId: string, userId: string): Promise<void> {
    await this.#ask(
      '/subscriptions/{blockId}/{userId}',
      { blockId, userId },
      { method: 'DELETE' },
      workspaceId,
    );
  }
}
