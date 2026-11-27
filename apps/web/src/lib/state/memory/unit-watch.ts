/**
 * What the reader watches in the in-memory backend (`Backend.watches`, and the
 * units' of `Backend.course`): a card, a page or a unit is watched by who
 * wrote it or commented on it, in the shared workspace, unless they stopped;
 * and by whoever started.
 */
import type { WatchedKind, Watches } from '../watch-types.ts';

import { reads } from './container-access.ts';
import { fail } from './refuse.ts';
import { checkWorkspace } from './spaces.ts';
import type { World } from './world.ts';

const WATCHED: ReadonlySet<string> = new Set<WatchedKind>(['card', 'page', 'unit']);

export function setUnitWatch(
  world: World,
  userId: string,
  unitId: string,
  watching: boolean,
): void {
  let byUnit = world.unitWatchers.get(userId);
  if (!byUnit) {
    byUnit = new Map();
    world.unitWatchers.set(userId, byUnit);
  }
  byUnit.set(unitId, watching);
}

export function memoryWatchingUnits(
  world: World,
  sessionUserId: string,
  workspaceId: string,
  userId: string,
  only: WatchedKind | 'any' = 'unit',
): string[] {
  checkWorkspace(world, sessionUserId, workspaceId);
  if (userId !== sessionUserId) fail(404, 'no such user');
  const explicit = world.unitWatchers.get(userId);
  const ids: string[] = [];
  for (const block of world.blocks.values()) {
    if (!WATCHED.has(block.type) || (only !== 'any' && block.type !== only)) continue;
    const container = world.containers.get(block.containerId);
    if (container?.workspaceId !== workspaceId || !reads(world, userId, container.id)) {
      continue;
    }
    const override = explicit?.get(block.id);
    if (override === false) continue;
    const auto =
      workspaceId === world.workspace.id &&
      (block.createdBy === userId ||
        world
          .childrenOf(block.id)
          .some((child) => child.type === 'comment' && child.createdBy === userId));
    if (override === true || auto) ids.push(block.id);
  }
  return ids;
}

export function memoryWatchUnit(
  world: World,
  sessionUserId: string,
  workspaceId: string,
  unitId: string,
  userId: string,
): void {
  checkWorkspace(world, sessionUserId, workspaceId);
  if (userId !== sessionUserId) {
    fail(403, 'a user subscribes for themselves only', { field: 'userId' });
  }
  const block = world.blocks.get(unitId);
  if (
    !block ||
    world.containers.get(block.containerId)?.workspaceId !== workspaceId ||
    !reads(world, sessionUserId, block.containerId) ||
    !WATCHED.has(block.type)
  ) {
    fail(404, 'no such block');
  }
  setUnitWatch(world, userId, unitId, true);
}

export function memoryUnwatchUnit(
  world: World,
  sessionUserId: string,
  workspaceId: string,
  unitId: string,
  userId: string,
): void {
  checkWorkspace(world, sessionUserId, workspaceId);
  if (userId !== sessionUserId) fail(404, 'no such subscription');
  setUnitWatch(world, userId, unitId, false);
}

/** `Backend.watches` of the world: of what the reader `of.userId` watches. */
export class MemoryWatches implements Watches {
  readonly #of: { readonly world: World; readonly userId: string };
  readonly #enter: () => Promise<void>;

  constructor(of: { readonly world: World; readonly userId: string }, enter: () => Promise<void>) {
    this.#of = of;
    this.#enter = enter;
  }

  async watching(workspaceId: string, userId: string): Promise<string[]> {
    await this.#enter();
    const { world, userId: me } = this.#of;
    return memoryWatchingUnits(world, me, workspaceId, userId, 'any');
  }

  async watch(workspaceId: string, blockId: string, _: WatchedKind, userId: string): Promise<void> {
    await this.#enter();
    memoryWatchUnit(this.#of.world, this.#of.userId, workspaceId, blockId, userId);
  }

  async unwatch(workspaceId: string, blockId: string, userId: string): Promise<void> {
    await this.#enter();
    memoryUnwatchUnit(this.#of.world, this.#of.userId, workspaceId, blockId, userId);
  }
}
