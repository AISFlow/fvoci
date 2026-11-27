/**
 * The versions of the bodies of cards, and of the pages of wikis, as the
 * server keeps them (`getCardBodyVersions`, `getPageBodyVersions`), for the
 * backend in memory:
 *
 * - the body a card has before somebody writes in its room is kept first
 *   (by nobody, as of the card's last change, when nobody is known to have
 *   written it), and what is written in the room is kept when those who
 *   wrote it have left it (whoever stays and wrote nothing), and when the
 *   room writes the card after the body has been written on for ten minutes;
 * - a body written whole is kept, by its writer, after the body it was
 *   written over; a version restored is such a write, and what it wrote is
 *   kept as restored;
 * - an editor asks to keep the body now (`keep`): kept by hand, unless it is
 *   the newest version already or one was kept by hand within a minute;
 * - a body that is the newest version already is not kept again, no body is
 *   no first version, and the newest fifty are kept.
 *
 * Unlike the server, two bodies are the same when they are written the
 * same (their members in the same order), and the versions of a card that
 * is deleted for good stay for the life of the world.
 */
import type { DocNode } from '../../editor/schema.ts';
import type {
  BodyVersion,
  BodyVersionCause,
  BodyVersionContent,
  BodyVersions,
  KeptBodyVersion,
} from '../body-versions.ts';

import { BackendError } from '../problems.ts';

import { memberRole } from './container-access.ts';
import { checkedBody } from './bodies.ts';
import { fail } from './refuse.ts';
import { changeOf, copy } from './shapes.ts';
import type { StoredCard, World } from './world.ts';

/** How many versions of a body are kept (the server's `collab_body_versions`). */
export const VERSIONS_KEPT = 50;
/** How long a body is written on before a version of it is kept (`collab_version_minutes`). */
export const VERSION_EVERY = 10 * 60_000;
/** How long after a version kept by hand no other is kept by hand (the server's `BY_HAND_FLOOR`). */
export const BY_HAND_FLOOR = 60_000;

interface Kept {
  version: number;
  keptAt: number;
  editors: string[];
  body: DocNode | null;
  cause: BodyVersionCause;
}

/** What is kept of one card's body. */
export interface KeptOfCard {
  /** The oldest first. */
  versions: Kept[];
  /** Who wrote the body since its newest version. */
  unkept: Set<string>;
  /** When what is written on is kept next; undefined while nobody wrote since the newest version. */
  due: number | undefined;
}

function keptOf(world: World, cardId: string): KeptOfCard {
  let kept = world.bodyVersions.get(cardId);
  if (!kept) {
    kept = { versions: [], unkept: new Set(), due: undefined };
    world.bodyVersions.set(cardId, kept);
  }
  return kept;
}

function same(one: DocNode | null, other: DocNode | null): boolean {
  return JSON.stringify(one) === JSON.stringify(other);
}

/**
 * Keeps a card's body as its newest version, by those who wrote since the
 * one before and `writtenBy`; not a body that is the newest version
 * already, and not no body as the first.
 */
function keep(
  world: World,
  card: StoredCard,
  writtenBy?: string,
  cause: BodyVersionCause = 'auto',
): void {
  const kept = keptOf(world, card.id);
  const body = card.body ?? null;
  const newest = kept.versions.at(-1);
  const editors = new Set(kept.unkept);
  kept.unkept.clear();
  kept.due = undefined;
  if (newest ? same(newest.body, body) : body === null) return;
  if (writtenBy !== undefined) editors.add(writtenBy);
  kept.versions.push({
    version: (newest?.version ?? 0) + 1,
    // Nobody is known to have written it: it is as old as the card's last change at the
    // latest. One asked for is as old as the asking.
    keptAt: editors.size === 0 && cause !== 'manual' ? card.updatedAt : world.now(),
    editors: [...editors].sort(),
    body: copy(body),
    cause,
  });
  kept.versions.splice(0, Math.max(0, kept.versions.length - VERSIONS_KEPT));
}

/**
 * A change of `userId` is taken into a card's room: the body before those who
 * write now began is kept first, and, as the server does, the body before the
 * first change after the period, when the card holds all its room took
 * (`holdsAll`); otherwise it is kept as the room next writes it (`roomWrote`).
 */
export function typedBy(
  world: World,
  card: StoredCard,
  userId: string,
  holdsAll: () => boolean,
): void {
  const kept = keptOf(world, card.id);
  if (kept.due === undefined || (Date.now() >= kept.due && holdsAll())) {
    keep(world, card);
    kept.due = Date.now() + VERSION_EVERY;
  }
  kept.unkept.add(userId);
}

/** A card's room wrote its body: kept, once it has been written on for long enough. */
export function roomWrote(world: World, card: StoredCard): void {
  const due = keptOf(world, card.id).due;
  if (due !== undefined && Date.now() >= due) keep(world, card);
}

/** Whether somebody wrote a card's body since its newest version, and none of `staying`, the users still in its room to write. */
export function leftByWriters(world: World, cardId: string, staying: string[]): boolean {
  const unkept = keptOf(world, cardId).unkept;
  return unkept.size > 0 && !staying.some((userId) => unkept.has(userId));
}

/** Those who wrote have left a card's room, and its body is written: what they wrote is kept. */
export function writersLeft(world: World, card: StoredCard): void {
  if (keptOf(world, card.id).unkept.size > 0) keep(world, card);
}

/**
 * Writes a card's body whole (`write` does, to `body`), as `userId`: the
 * body before is kept, then the body written, as the writer's (as `cause`:
 * a version gone back to is kept as restored). Nothing is kept around a
 * write of the body the card has.
 */
export function writtenWhole(
  world: World,
  card: StoredCard,
  body: DocNode | null,
  userId: string,
  write: () => void,
  cause: BodyVersionCause = 'auto',
): void {
  const changes = !same(card.body ?? null, body);
  if (changes) keep(world, card);
  write();
  if (changes) keep(world, card, userId, cause);
}

/**
 * The body of a version as it is written to its card again: held to what a
 * body a client sends is held to, and refused as a conflict (409) when it
 * may not be stored any more or names a file (of an image or a file block)
 * that is gone.
 */
function storable(world: World, containerId: string, body: DocNode | null): DocNode | null {
  try {
    return checkedBody(world, containerId, body);
  } catch (error) {
    if (!(error instanceof BackendError)) throw error;
    fail(409, `the version cannot be restored: ${error.message}`);
  }
}

/**
 * The versions of a world's cards, or of its wikis' pages (`of`), read and
 * restored as one of its users: the board's or the wiki's members read them,
 * its editors restore them.
 */
export class MemoryBodyVersions implements BodyVersions {
  readonly #world: World;
  readonly #userId: string;
  /** The wait every call of the backend starts with, and the failure a test asked for. */
  readonly #enter: () => Promise<void>;
  readonly #of: 'card' | 'page' | 'unit' | 'entry';

  constructor(
    world: World,
    userId: string,
    enter: () => Promise<void>,
    of: 'card' | 'page' | 'unit' | 'entry' = 'card',
  ) {
    this.#world = world;
    this.#userId = userId;
    this.#enter = enter;
    this.#of = of;
  }

  get #kind(): 'board' | 'wiki' | 'course' | 'diary' {
    if (this.#of === 'page') return 'wiki';
    if (this.#of === 'unit') return 'course';
    if (this.#of === 'entry') return 'diary';
    return 'board';
  }

  async list(containerId: string, cardId: string): Promise<BodyVersion[]> {
    await this.#enter();
    return this.#described(this.#card(containerId, cardId)).map(
      ({ body: _, ...version }) => version,
    );
  }

  async read(containerId: string, cardId: string, version: number): Promise<BodyVersionContent> {
    await this.#enter();
    const kept = this.#described(this.#card(containerId, cardId));
    return kept.find((one) => one.version === version) ?? fail(404, 'no such version');
  }

  async restore(containerId: string, cardId: string, version: number): Promise<void> {
    await this.#enter();
    const world = this.#world;
    const card = this.#editable(containerId, cardId, 'restore');
    const kept = keptOf(world, cardId).versions.find((one) => one.version === version);
    if (!kept) fail(404, 'no such version');
    // What the room's editors typed is in the card first, and is kept with the body before.
    world.bodyFlushed?.(cardId);
    if (same(card.body ?? null, kept.body)) return;
    const body = storable(world, containerId, kept.body);
    writtenWhole(
      world,
      card,
      body,
      this.#userId,
      () => {
        card.body = body;
        card.updatedAt = card.bodyUpdatedAt = world.now();
        card.modifiedBy = this.#userId;
      },
      'restored',
    );
    world.bodyWritten?.(cardId);
    world.tellContainer(containerId, changeOf(containerId, card));
  }

  /**
   * Keeps a card's body as a version now, for who edits the board, as the
   * server does (`keepCardBodyVersion`): what the room's editors typed is in
   * the card first; a body that is the newest version already is not kept
   * again, nor one within a minute of a version kept by hand. Kept, it names
   * who wrote since the version before (not who asked), and the interval
   * counts again from the next change.
   */
  async keep(containerId: string, cardId: string): Promise<KeptBodyVersion> {
    await this.#enter();
    const world = this.#world;
    const card = this.#editable(containerId, cardId, 'keep');
    const kept = keptOf(world, cardId);
    // As the server: the room's body is written for this version, not kept on its own first
    // though the interval is up; and the interval stays as it was if none is kept.
    const due = kept.due;
    kept.due = undefined;
    world.bodyFlushed?.(cardId);
    const newest = kept.versions.at(-1);
    if (newest ? same(newest.body, card.body ?? null) : (card.body ?? null) === null) {
      // Kept already: nobody wrote since it, and the interval waits for the next change.
      kept.unkept.clear();
      return { outcome: 'unchanged', version: this.#listed(card, newest?.version) };
    }
    const byHand = kept.versions.findLast((one) => one.cause === 'manual');
    if (byHand && world.now() - byHand.keptAt <= BY_HAND_FLOOR) {
      kept.due = due;
      return { outcome: 'recent', version: this.#listed(card, byHand.version) };
    }
    keep(world, card, undefined, 'manual');
    return { outcome: 'kept', version: this.#listed(card, kept.versions.at(-1)?.version) };
  }

  /**
   * A card of the board (a page of the wiki), for who edits it: `what` is
   * refused to the others. As the server: a card is looked for first, a
   * page's wiki asked first.
   */
  #editable(containerId: string, cardId: string, what: 'restore' | 'keep'): StoredCard {
    const role = memberRole(this.#world, this.#userId, containerId, this.#kind);
    const refuse = (): void => {
      if (role !== 'admin' && role !== 'editor') {
        fail(403, `access denied to ${what} a version of the ${this.#of}'s body`);
      }
    };
    if (this.#of === 'page' || this.#of === 'unit' || this.#of === 'entry') refuse();
    const card = this.#card(containerId, cardId);
    refuse();
    return card;
  }

  /** One version of a card as the list has it; none for no number, or one it has no more. */
  #listed(card: StoredCard, version: number | undefined): BodyVersion | null {
    const found = this.#described(card).find((one) => one.version === version);
    if (!found) return null;
    const { body: _, ...listed } = found;
    return listed;
  }

  /** A card of the board (a page of the wiki), for a member of it: the versions are theirs to read. */
  #card(containerId: string, cardId: string): StoredCard {
    memberRole(this.#world, this.#userId, containerId, this.#kind);
    const block = this.#world.blocks.get(cardId);
    if (block?.type !== this.#of || block.containerId !== containerId)
      fail(404, `no such ${this.#of}`);
    return block as StoredCard;
  }

  /**
   * The versions of a card, the newest first, the newest with the card's
   * body marked: none while the card's room holds what the card does not yet.
   */
  #described(card: StoredCard): BodyVersionContent[] {
    const kept = [...keptOf(this.#world, card.id).versions].reverse();
    const current = this.#world.bodyUnwritten?.(card.id)
      ? undefined
      : kept.find((one) => same(one.body, card.body ?? null))?.version;
    return kept.map((one) => ({
      ...copy(one),
      isCurrent: one.version === current,
    }));
  }
}
