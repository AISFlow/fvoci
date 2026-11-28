/**
 * One value of one property put on many cards of a board at once, as the
 * bar of a selection does it (features/views/BulkBar.svelte): one request
 * (`batchPatchCards`), which the server stores all or none, and one step of
 * the history. A function of the board store, which it is given.
 *
 * The cards show the value at once. When the request is refused nothing was
 * stored, and every card that still shows what it wrote is shown again as it
 * was; a card that something else changed meanwhile keeps that change, and
 * so does one of which a newer version came from the server (its `updatedAt`
 * is not the one it had when the value was shown), whatever it holds. An
 * answer that was lost (status 0) may have been stored all the same: the
 * cards are read again, and when they hold what was written, that reading
 * is the answer.
 *
 * A card that repeats and is made done makes its next card on the server: the
 * one who did it is told (`NextCardNotices`), and undo makes it repeat again
 * and takes the next card away, as it does for a single card.
 *
 * Undo gives each card back its own value. The route sets one value on all
 * the cards it names, so undo sends one request for each value the cards had
 * (no value at all is one of them); each is a value set, not a change, and
 * may be sent again. Undo and redo leave out the cards that are gone by then.
 *
 * A batch made over another one under way on the same cards waits for its
 * answer before it is sent; where that one was refused, what the later one
 * gives back is what was stored, not the value refused (`overRefused`). A
 * card that shows the value only because a batch under way put it there is
 * part of the later one all the same: that batch may yet be refused.
 *
 * These writes do not go through the store's writes of single cards: a
 * version of a card that arrives while one is under way is shown, and the
 * answer, newer, then takes its place. A single card's write made over a
 * batch under way is not waited for.
 */
import type { PropertyValue } from '../logic/wire.ts';

import type { CardPropertiesPatch } from './authority-types.ts';
import { isBackendError, type CardSummary } from './backend.ts';
import { withCardProperty } from './board-parts.ts';
import { performBoardAction } from './board-sync.ts';
import type { BoardStore } from './board.svelte.ts';
import { sameValue } from './inverse-values.ts';
import {
  giveBackNextCards,
  nextCardNotices,
  noteNextCards,
  type MadeNext,
} from './next-cards.svelte.ts';
import { applyCardPatch, cardPatches, isEmptyCardPatch } from './patches.ts';

/** A card's part of a batch: the values to show and send, and those to show again if it fails. */
interface CardWrite {
  cardId: string;
  patch: CardPropertiesPatch;
  restore: CardPropertiesPatch;
}

/** A call of `writeCards` under way: its writes, and the cards it could not store once it answered. */
interface Writing {
  readonly writes: readonly CardWrite[];
  readonly refused: Promise<ReadonlySet<string>>;
}

/** The batches under way on each store's cards, in the order they were made. */
const underWay = new WeakMap<BoardStore, Set<Writing>>();

/** The values a patch sets on a card, by property; a cleared one is undefined. */
function valuesOf(patch: CardPropertiesPatch): Map<string, PropertyValue | undefined> {
  const values = new Map<string, PropertyValue | undefined>(
    Object.entries(patch.updatedProperties ?? {}),
  );
  for (const id of patch.deletedProperties ?? []) values.set(id, undefined);
  return values;
}

/** The patch that sets `values`: a value undefined is cleared. */
function patchOf(values: ReadonlyMap<string, PropertyValue | undefined>): CardPropertiesPatch {
  const patch: CardPropertiesPatch = {};
  for (const [id, value] of values) {
    if (value === undefined) (patch.deletedProperties ??= []).push(id);
    else (patch.updatedProperties ??= {})[id] = value;
  }
  return patch;
}

/**
 * `write` was made over `earlier`, on the same card, and `earlier` was
 * refused: where `write` would give back what `earlier` showed, it gives
 * back what `earlier` would have, which is what was stored.
 */
function overRefused(write: CardWrite, earlier: CardWrite): void {
  const shown = valuesOf(earlier.patch);
  const stored = valuesOf(earlier.restore);
  const restore = valuesOf(write.restore);
  let changed = false;
  for (const [id, value] of restore) {
    if (!shown.has(id) || !sameValue(shown.get(id), value)) continue;
    restore.set(id, stored.get(id));
    changed = true;
  }
  if (changed) write.restore = patchOf(restore);
}

/** Puts the changed cards on screen in one assignment. */
function show(store: BoardStore, changed: ReadonlyMap<string, CardSummary>): void {
  if (changed.size === 0) return;
  store.cards = store.cards.map((card) => changed.get(card.id) ?? card);
}

/**
 * Whether the server sent `card` anew since the writes were shown (`since`,
 * its `updatedAt` then): it then shows what was stored.
 */
function sentAnew(card: CardSummary, since: ReadonlyMap<string, number>): boolean {
  return card.updatedAt !== since.get(card.id);
}

/**
 * The cards that still show what `writes` put on them, shown again as before;
 * not one that the server sent anew since ({@link sentAnew}).
 */
function takeBack(
  store: BoardStore,
  writes: readonly CardWrite[],
  since: ReadonlyMap<string, number>,
): void {
  const changed = new Map<string, CardSummary>();
  for (const write of writes) {
    const shown = store.card(write.cardId);
    if (!shown || sentAnew(shown, since)) continue;
    const own: CardPropertiesPatch = {};
    const restore = valuesOf(write.restore);
    for (const [id, value] of valuesOf(write.patch)) {
      if (!sameValue(shown.fields.properties[id], value)) continue;
      const before = restore.get(id);
      if (before === undefined) (own.deletedProperties ??= []).push(id);
      else (own.updatedProperties ??= {})[id] = before;
    }
    if (!isEmptyCardPatch(own)) changed.set(shown.id, applyCardPatch(shown, own));
  }
  show(store, changed);
}

/** Whether every card read holds the values that were written on it. */
function holds(read: readonly CardSummary[], writes: readonly CardWrite[]): boolean {
  return writes.every((write) => {
    const card = read.find((one) => one.id === write.cardId);
    if (!card) return false;
    return [...valuesOf(write.patch)].every(([id, value]) =>
      sameValue(card.fields.properties[id], value),
    );
  });
}

/** Sends one batch: the answer, or what the board holds when the answer was lost and it holds the write. */
async function send(
  store: BoardStore,
  writes: readonly CardWrite[],
  quiet: boolean,
): Promise<CardSummary[]> {
  const { backend } = store.workspace;
  const patch = writes[0]?.patch ?? {};
  const ids = writes.map((write) => write.cardId);
  try {
    return await backend.batchPatchCards(store.boardId, ids, patch, quiet ? { quiet } : undefined);
  } catch (error) {
    if (!isBackendError(error) || error.status !== 0) throw error;
    const read = await backend.cards(store.boardId).catch(() => undefined);
    if (!read || !holds(read, writes)) throw error;
    return read.filter((card) => ids.includes(card.id));
  }
}

/** The writes grouped by the patch they send: one request each. */
function batchesOf(writes: readonly CardWrite[]): CardWrite[][] {
  const batches: CardWrite[][] = [];
  for (const write of writes) {
    const batch = batches.find((one) => sameValue(one[0]?.patch, write.patch));
    if (batch) batch.push(write);
    else batches.push([write]);
  }
  return batches;
}

/** Waits for the batches under way on the cards of `writes`, and gives back what they did not store. */
async function after(earlier: readonly Writing[], writes: readonly CardWrite[]): Promise<void> {
  for (const one of earlier) {
    const refused = await one.refused;
    for (const write of writes) {
      if (!refused.has(write.cardId)) continue;
      for (const before of one.writes)
        if (before.cardId === write.cardId) overRefused(write, before);
    }
  }
}

/**
 * Sends the batches of `writes`: what is answered is shown, what is refused
 * taken back. Says which cards were refused and still show what the refused
 * batch wrote: one the server sent anew meanwhile ({@link sentAnew}) is not
 * named, so that a batch after it takes back to what the server sent.
 */
async function sendAll(
  store: BoardStore,
  writes: readonly CardWrite[],
  quiet: boolean,
  since: ReadonlyMap<string, number>,
): Promise<{ refused: Set<string>; error?: Error }> {
  const batches = batchesOf(writes);
  const answers = await Promise.allSettled(batches.map((batch) => send(store, batch, quiet)));
  const refused = new Set<string>();
  let error: Error | undefined;
  answers.forEach((answer, at) => {
    if (answer.status === 'fulfilled') {
      for (const card of answer.value) store.receiveCard(card);
      return;
    }
    const batch = batches[at] ?? [];
    takeBack(store, batch, since);
    for (const write of batch) {
      const shown = store.card(write.cardId);
      if (!shown || !sentAnew(shown, since)) refused.add(write.cardId);
    }
    // What a backend throws is a `BackendError`, or the error of a failed reading.
    error ??= answer.reason as Error;
  });
  return { refused, error };
}

/**
 * Shows the writes at once and sends them, a batch for each patch, once the
 * batches under way on the same cards have answered; a batch refused is
 * taken back on its cards, and its error is thrown once all have answered.
 */
async function writeCards(
  store: BoardStore,
  writes: readonly CardWrite[],
  quiet: boolean,
): Promise<void> {
  const present = writes.filter((write) => store.card(write.cardId) !== undefined);
  const shown = new Map<string, CardSummary>();
  const since = new Map<string, number>();
  for (const write of present) {
    const card = store.card(write.cardId);
    if (!card) continue;
    shown.set(card.id, applyCardPatch(card, write.patch));
    since.set(card.id, card.updatedAt);
  }
  show(store, shown);

  const cards = new Set(present.map((write) => write.cardId));
  const all = underWay.get(store) ?? new Set<Writing>();
  underWay.set(store, all);
  const earlier = [...all].filter((one) => one.writes.some((write) => cards.has(write.cardId)));
  let answered: (refused: ReadonlySet<string>) => void = () => undefined;
  const mine: Writing = {
    writes: present,
    refused: new Promise((resolve) => {
      answered = resolve;
    }),
  };
  all.add(mine);
  let refused: ReadonlySet<string> = new Set();
  try {
    await after(earlier, present);
    const sent = await sendAll(store, present, quiet, since);
    refused = sent.refused;
    if (sent.error) throw sent.error;
  } finally {
    answered(refused);
    all.delete(mine);
  }
}

/** The cards of the store on which a batch under way writes `propertyId`. */
function writingProperty(store: BoardStore, propertyId: string): Set<string> {
  const cards = new Set<string>();
  for (const one of underWay.get(store) ?? []) {
    for (const write of one.writes)
      if (valuesOf(write.patch).has(propertyId)) cards.add(write.cardId);
  }
  return cards;
}

/**
 * Sets `propertyId` to `value` on the cards of `cardIds` (an empty value
 * clears it), as one step of the history. The cards that have that value
 * already, templates and cards that are not there are left out; when none
 * is left, nothing is done. A card that shows the value as a batch under way
 * writes it is not left out, since that one may be refused. The server takes
 * at most `CARD_BATCH_LIMIT` cards in one request: more are refused, and
 * nothing changes.
 */
export async function setCardsProperty(
  store: BoardStore,
  cardIds: readonly string[],
  propertyId: string,
  value: PropertyValue | undefined,
): Promise<void> {
  const forward: CardWrite[] = [];
  const pending = writingProperty(store, propertyId);
  for (const cardId of new Set(cardIds)) {
    const card = store.card(cardId);
    if (!card || card.fields.isTemplate) continue;
    const [patch, restore] = cardPatches(withCardProperty(card, propertyId, value), card);
    if (!isEmptyCardPatch(patch)) forward.push({ cardId, patch, restore });
    else if (pending.has(cardId)) {
      // The value it shows is that batch's: sent again, it gives back that value until
      // that batch is refused, and then what was stored (`overRefused`).
      const shown = patchOf(new Map([[propertyId, card.fields.properties[propertyId]]]));
      forward.push({ cardId, patch: shown, restore: shown });
    }
  }
  if (forward.length === 0) return;
  // Read when undone: a batch refused under this one may have corrected what it gives back.
  const backward = (): CardWrite[] =>
    forward.map(({ cardId, patch, restore }) => ({ cardId, patch: restore, restore: patch }));
  let said = false;
  /** The next cards the last redo made, by the card it completed. */
  const made = new Map<string, MadeNext>();
  const performing = performBoardAction(
    store,
    async () => {
      const before = new Map(forward.map(({ cardId }) => [cardId, store.card(cardId)]));
      await writeCards(store, forward, said);
      said = true;
      noteNextCards(store, before, made);
    },
    async () => {
      await writeCards(store, backward(), true);
      await giveBackNextCards(store, made);
    },
    'changeProperty',
  );
  const checkpoint = store.workspace.undo.currentCheckpoint;
  await performing;
  if (made.size > 0) {
    void nextCardNotices(store.workspace).say(store, [...made.values()], checkpoint);
  }
}

/** Clears `propertyId` on the cards of `cardIds`, as one step of the history. */
export function clearCardsProperty(
  store: BoardStore,
  cardIds: readonly string[],
  propertyId: string,
): Promise<void> {
  return setCardsProperty(store, cardIds, propertyId, undefined);
}
