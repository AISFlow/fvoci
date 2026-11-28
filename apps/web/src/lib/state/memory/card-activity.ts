/**
 * The activity of cards for `MemoryBackend`, as the server keeps it
 * (`crates/store/src/card_events.rs`): a line for a card's making, for each
 * change of its title, a property's value, its parent and whether it is a
 * template, and for its coming back from the trash. The same person changing
 * the same thing again within five minutes changes their line, and changing
 * it back takes the line away; a card keeps its 500 newest lines. A copy
 * says only that it was made.
 */
import type { Card } from '../../logic/wire.ts';
import type { CardActivity, CardActivityKind, CardActivityLine } from '../activity-types.ts';
import type { Paging } from '../trash-reads.ts';

import { blockInBoard } from './block-ops.ts';
import { memberRole } from './container-access.ts';
import type { MemoryContainerContext } from './container-ops.ts';
import { copy } from './shapes.ts';
import type { StoredBlock, World } from './world.ts';

const MERGE_WITHIN = 5 * 60 * 1000;
const KEPT_PER_CARD = 500;
const DEFAULT_PER_PAGE = 50;

interface Change {
  kind: CardActivityKind;
  propertyId: string | null;
  before: unknown;
  after: unknown;
}

const same = (first: unknown, second: unknown): boolean =>
  JSON.stringify(first ?? null) === JSON.stringify(second ?? null);

/** What changed of a card, in the server's order: title, parent, template, then the values by property. */
function changesOf(before: Card, after: Card): Change[] {
  const changes: Change[] = [];
  const push = (kind: CardActivityKind, was: unknown, is: unknown, propertyId = null): void => {
    if (!same(was, is)) changes.push({ kind, propertyId, before: was ?? null, after: is ?? null });
  };
  push('title', before.title, after.title);
  push('parent', before.parentId || null, after.parentId || null);
  push('template', Boolean(before.fields.isTemplate), Boolean(after.fields.isTemplate));
  const was = before.fields.properties;
  const is = after.fields.properties;
  for (const id of [...new Set([...Object.keys(was), ...Object.keys(is)])].sort()) {
    if (!same(was[id], is[id])) {
      changes.push({
        kind: 'property',
        propertyId: id,
        before: copy(was[id] ?? null),
        after: copy(is[id] ?? null),
      });
    }
  }
  return changes;
}

function linesOf(world: World, cardId: string): CardActivityLine[] {
  let lines = world.cardEvents.get(cardId);
  if (!lines) {
    lines = [];
    world.cardEvents.set(cardId, lines);
  }
  return lines;
}

function add(world: World, cardId: string, line: Omit<CardActivityLine, 'id'>): void {
  const lines = linesOf(world, cardId);
  lines.push({ id: world.newId(), ...line });
  if (lines.length > KEPT_PER_CARD) lines.splice(0, lines.length - KEPT_PER_CARD);
}

const isCard = (block: StoredBlock): block is StoredBlock & Card => block.type === 'card';

/** A card was made: a new one, a copy, the next of one that repeats. */
export function noteCardMade(world: World, block: StoredBlock, userId: string): void {
  if (!isCard(block)) return;
  const line = { kind: 'created', propertyId: null, before: null, after: null } as const;
  add(world, block.id, {
    ...line,
    actorId: userId || null,
    actorTokenName: null,
    createdAt: block.createdAt,
    updatedAt: block.createdAt,
  });
}

/** A card was changed from `before` to `after`, by `userId`. */
export function noteCardChanged(
  world: World,
  before: StoredBlock,
  after: StoredBlock,
  userId: string,
): void {
  if (!isCard(before) || !isCard(after)) return;
  const now = after.updatedAt;
  const lines = linesOf(world, after.id);
  for (const change of changesOf(before, after)) {
    const newest = lines.findLast(
      (line) => line.kind === change.kind && line.propertyId === change.propertyId,
    );
    if (newest?.actorId === (userId || null) && now - newest.updatedAt <= MERGE_WITHIN) {
      if (same(newest.before, change.after)) lines.splice(lines.indexOf(newest), 1);
      else Object.assign(newest, { after: change.after, updatedAt: now });
      continue;
    }
    add(world, after.id, {
      ...change,
      actorId: userId || null,
      actorTokenName: null,
      createdAt: now,
      updatedAt: now,
    });
  }
}

/** Cards came back from the trash. */
export function noteCardsRestored(
  world: World,
  blocks: readonly StoredBlock[],
  userId: string,
): void {
  const now = world.now();
  for (const block of blocks.filter(isCard)) {
    const line = { kind: 'restored', propertyId: null, before: null, after: null } as const;
    add(world, block.id, {
      ...line,
      actorId: userId || null,
      actorTokenName: null,
      createdAt: now,
      updatedAt: now,
    });
  }
}

export class MemoryCardActivity implements CardActivity {
  readonly #ctx: MemoryContainerContext;
  readonly #enter: () => Promise<void>;

  constructor(ctx: MemoryContainerContext, enter: () => Promise<void>) {
    this.#ctx = ctx;
    this.#enter = enter;
  }

  async list(boardId: string, cardId: string, paging?: Paging): Promise<CardActivityLine[]> {
    await this.#enter();
    const { world, userId } = this.#ctx;
    memberRole(world, userId, boardId);
    blockInBoard(world, boardId, cardId, 'card');
    const lines = [...(world.cardEvents.get(cardId) ?? [])].sort(
      (a, b) => b.updatedAt - a.updatedAt || (a.id < b.id ? 1 : a.id > b.id ? -1 : 0),
    );
    const perPage = paging?.perPage ?? DEFAULT_PER_PAGE;
    const start = (paging?.page ?? 0) * perPage;
    return copy(lines.slice(start, start + perPage));
  }
}
