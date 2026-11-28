/**
 * Cards that repeat for `MemoryBackend`, as the server makes them
 * (`crates/store/src/card_recurrence.rs`): only a board with a rule of done
 * and a due date lets a card repeat (400 `recurrence_needs_board_rules`,
 * but for a copy, which drops it); the next card is named by the server
 * alone (`recurredTo`); the change that makes a card that repeats done makes
 * its next one, not done, due one period on in the completer's day, with its
 * tasks unchecked, and hands it the recurrence with the first date of its
 * chain, which a change of how the card repeats or of its due date takes
 * away. A template does not repeat.
 */
import type { DocNode } from '../../editor/schema.ts';
import { valuesOfCopy } from '../../logic/dependencies.ts';
import {
  anew,
  boardLetsCardsRepeat,
  duePropertyOf,
  handedOn,
  isDoneValue,
  nextDueValue,
  notDoneValue,
  sameRecurrence,
  type Recurrence,
} from '../../logic/recurrence.ts';
import {
  cardProperty,
  cardRecurrence,
  type Container,
  type PropertyValue,
} from '../../logic/wire.ts';
import { BackendError, problemOf, refusedMember } from '../backend.ts';

import { settingsOf } from './account.ts';
import { noteCardMade } from './card-activity.ts';
import { numberBlock } from './card-numbers.ts';
import { newStoredBlock, type MemoryContainerContext } from './container-ops.ts';
import { copy } from './shapes.ts';
import type { StoredCard } from './world.ts';

const NEEDS_RULES = 'a card repeats on a board with a rule of done and a due date property';

function refused(): never {
  throw new BackendError(400, {
    ...problemOf(400, NEEDS_RULES),
    reason: 'recurrence_needs_board_rules',
    type: '/problems/recurrence-needs-board-rules',
    errors: [refusedMember('recurrence', NEEDS_RULES)],
  });
}

/** A recurrence on a card of a board that does not let it: refused, or dropped from a copy. */
function refuseOrDrop(card: StoredCard, quiet: boolean): void {
  if (!quiet) refused();
  delete card.fields.recurrence;
}

/** A new card names no next card yet, and repeats only where its board lets it. */
export function settleNewCard(board: Container, card: StoredCard, quiet: boolean): void {
  delete card.fields.recurredTo;
  if (card.fields.recurrence && !boardLetsCardsRepeat(board)) refuseOrDrop(card, quiet);
}

/** The next card a change plans: its identifier, which the completed card names already. */
export interface NextCard {
  id: string;
  recurrence: Recurrence;
  /** The value of the rule's property before the card was done. */
  before?: PropertyValue;
}

/**
 * How a changed card repeats (`sent`), as for the first date of its chain:
 * the one it had while it repeats as it did, the one a change gives back to a
 * card that did not repeat (an undone completion), none once how it repeats
 * or its due date is changed.
 */
function chained(had: Recurrence | undefined, sent: Recurrence, dueChanged: boolean): Recurrence {
  if (dueChanged) return anew(sent);
  if (!had) return sent;
  if (!sameRecurrence(had, sent)) return anew(sent);
  return { ...anew(sent), ...(had.from ? { from: had.from } : {}) };
}

/**
 * Settles how a changed card repeats before it is stored: the next card is
 * the one the card named before, a recurrence set anew is checked and
 * repeats anew, and a change that makes a card that repeats done plans the
 * next card, which the card then names and hands its recurrence to.
 */
export function settleCardChange(
  ctx: MemoryContainerContext,
  board: Container,
  before: StoredCard,
  after: StoredCard,
): NextCard | undefined {
  if (before.fields.recurredTo) after.fields.recurredTo = before.fields.recurredTo;
  else delete after.fields.recurredTo;
  const sent = cardRecurrence(after);
  if (!sent) return undefined;
  const had = cardRecurrence(before);
  if (!sameRecurrence(had, sent)) {
    if (!boardLetsCardsRepeat(board)) {
      refuseOrDrop(after, false);
      return undefined;
    }
    delete after.fields.recurredTo;
  }
  const due = duePropertyOf(board);
  const dueChanged =
    due !== undefined &&
    JSON.stringify(cardProperty(before, due.id)) !== JSON.stringify(cardProperty(after, due.id));
  const recurrence = chained(had, sent, dueChanged);
  after.fields.recurrence = recurrence;
  const rule = board.doneWhen;
  if (after.fields.isTemplate || !rule || !boardLetsCardsRepeat(board)) return undefined;
  const done = (card: StoredCard): boolean =>
    isDoneValue(rule, cardProperty(card, rule.propertyId));
  if (done(before) || !done(after)) return undefined;
  const id = ctx.world.newId();
  delete after.fields.recurrence;
  after.fields.recurredTo = id;
  return { id, recurrence, before: cardProperty(before, rule.propertyId) };
}

function unchecked(node: DocNode): DocNode {
  const attrs = node.type === 'taskItem' ? { ...node.attrs, checked: false } : node.attrs;
  return {
    ...node,
    ...(attrs === undefined ? {} : { attrs }),
    ...(node.content ? { content: node.content.map(unchecked) } : {}),
  };
}

/** Makes the card after `completed`, as `settleCardChange` planned it, by the completer. */
export function makeNextCard(
  ctx: MemoryContainerContext,
  board: Container,
  completed: StoredCard,
  next: NextCard,
): StoredCard {
  const { world } = ctx;
  const zone = settingsOf(world, ctx.userId).timeZone;
  const timeZone = zone === 'auto' ? 'UTC' : zone;
  const onBoard = new Set(
    world
      .blocksOf(board.id)
      .filter((block) => block.type === 'card')
      .map((card) => card.id),
  );
  const values = valuesOfCopy(completed, board.cardProperties, onBoard);
  /** The values the next card starts with anew: none where it is undefined. */
  const fresh: Record<string, PropertyValue | undefined> = {};
  const rule = board.doneWhen;
  const ruled = rule && board.cardProperties.find((prop) => prop.id === rule.propertyId);
  if (rule && ruled) {
    fresh[rule.propertyId] = notDoneValue(ruled, rule, values[rule.propertyId], next.before);
  }
  const due = duePropertyOf(board);
  if (due) fresh[due.id] = nextDueValue(next.recurrence, values[due.id], world.now(), timeZone);
  const recurrence = handedOn(next.recurrence, due && values[due.id], timeZone);
  const properties = Object.fromEntries(
    Object.entries({ ...values, ...fresh }).filter(
      (entry): entry is [string, PropertyValue] => entry[1] !== undefined,
    ),
  );
  const block = newStoredBlock(ctx, board.id, completed.parentId, 'card');
  block.id = next.id;
  block.title = completed.title;
  block.fields = {
    icon: completed.fields.icon ?? '',
    isTemplate: false,
    properties,
    recurrence,
  };
  block.body = completed.body ? unchecked(copy(completed.body)) : null;
  numberBlock(world, block);
  world.blocks.set(block.id, block);
  noteCardMade(world, block, ctx.userId);
  return block as StoredCard;
}
