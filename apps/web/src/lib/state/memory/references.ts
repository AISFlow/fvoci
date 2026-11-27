/**
 * What blocks of the world in memory name of other blocks, as the server
 * handles it: the references a copy renames (`crates/model/src/block_ids.rs`),
 * and the card relation values a write is refused for
 * (`crates/store/src/card_values.rs`).
 *
 * A relation value (a property of the type `relation`) is null or a list of
 * cards of the same board, in the trash or not, each once, never the card
 * itself, at most RELATION_LIMIT. Only the values a write changes are
 * checked, and a card the value named before is not asked for again. A card
 * of another board is answered as one that is not there.
 */
import {
  isRelationLinksValue,
  parseRelationLink,
  parseRelationValue,
  RELATION_LIMIT,
  relationCardIds,
  type Container,
  type RelationLink,
} from '../../logic/wire.ts';
import { BackendError } from '../backend.ts';

import { renameNamed } from './linked.ts';
import type { StoredBlock, World } from './world.ts';

export { MemoryReferences } from './linked.ts';

type RelationReason =
  | 'relation_not_a_list'
  | 'relation_to_itself'
  | 'relation_repeated'
  | 'relation_too_many'
  | 'relation_not_a_card'
  | 'relation_card_not_found';

/** The server's title and words for each refusal; `{p}` is the property, quoted. */
const REFUSALS: Record<RelationReason, [string, string]> = {
  relation_not_a_list: [
    'Card relation that is no list',
    'the value of card relation {p} must be a list of relation links or null',
  ],
  relation_to_itself: [
    'Card relation to the card itself',
    'card relation {p} names the card itself',
  ],
  relation_repeated: ['Card named twice in a relation', 'card relation {p} names a card twice'],
  relation_too_many: [
    'Card relation names too many cards',
    `card relation {p} names more than ${String(RELATION_LIMIT)} cards`,
  ],
  relation_not_a_card: [
    'Card relation names no card',
    'card relation {p} names a block of the board that is no card',
  ],
  relation_card_not_found: [
    'Card relation names a card not on the board',
    'card relation {p} names a card that is not on the board',
  ],
};

/** Refuses as the server does: the reason, and the member of the request the values are in. */
function refuse(property: string, reason: RelationReason, member: string): never {
  const [title, words] = REFUSALS[reason];
  const detail = words.replace('{p}', JSON.stringify(property));
  throw new BackendError(400, {
    type: `/problems/${reason.replaceAll('_', '-')}`,
    title,
    status: 400,
    detail,
    reason,
    errors: [{ pointer: `/${member}`, detail, reason }],
    ...(reason === 'relation_too_many' ? { limit: RELATION_LIMIT } : {}),
  });
}

/** Refuses a relation link list written under a non-relation property. */
function refuseWrongProperty(property: string, member: string): never {
  const detail = `card property ${JSON.stringify(property)} is not a relation property, so its value cannot be a list of relation links`;
  throw new BackendError(400, {
    type: '/problems/bad-request',
    title: 'Bad request',
    status: 400,
    detail,
    errors: [{ pointer: `/${member}`, detail, reason: 'invalid' }],
  });
}

/** Refuses a malformed relation link object inside a list, matching Rust `InvalidRelationLink`. */
function refuseInvalidLink(property: string, member: string): never {
  const detail = `the relation link of card property ${JSON.stringify(property)} is refused`;
  throw new BackendError(400, {
    type: '/problems/bad-request',
    title: 'Bad request',
    status: 400,
    detail,
    errors: [{ pointer: `/${member}/${property}`, detail, reason: 'invalid' }],
  });
}

/** The relation properties of a board. */
function relationIds(board: Container): string[] {
  return board.cardProperties
    .filter((template) => template.type === 'relation')
    .map((template) => template.id);
}

function propertiesOf(block: StoredBlock | undefined): Record<string, unknown> {
  const properties = block?.fields.properties;
  return typeof properties === 'object' && properties !== null
    ? (properties as Record<string, unknown>)
    : {};
}

/** The type of the board's block of this identifier, in the trash or not. */
function typeOnBoard(world: World, boardId: string, id: string): string | undefined {
  const block = world.blocks.get(id);
  if (block) return block.containerId === boardId ? block.type : undefined;
  for (const deleted of world.deletedBlocks.values()) {
    if (deleted.containerId !== boardId) continue;
    const found = deleted.blocks.find((each) => each.id === id);
    if (found) return found.type;
  }
  return undefined;
}

/**
 * Refuses the relation values that `card` is written with and `before` (the
 * card as stored, if it is) did not have, as the server does, and normalizes
 * accepted relation links to `{ card, kind, lagDays }`. `member` is the member
 * of the request the values are in.
 */
export function checkRelations(
  world: World,
  board: Container,
  card: StoredBlock,
  before: StoredBlock | undefined,
  member: string,
): void {
  const values = propertiesOf(card);
  const previous = propertiesOf(before);
  const relIds = new Set(relationIds(board));
  for (const [property, value] of Object.entries(values)) {
    if (relIds.has(property)) continue;
    if (JSON.stringify(value) === JSON.stringify(previous[property])) continue;
    if (isRelationLinksValue(value)) refuseWrongProperty(property, member);
  }
  const normalized: [string, RelationLink[]][] = [];
  for (const property of relIds) {
    const value = values[property];
    if (value === undefined || value === null) continue;
    if (JSON.stringify(value) === JSON.stringify(previous[property])) continue;
    if (!Array.isArray(value)) refuse(property, 'relation_not_a_list', member);
    const items = value as unknown[];
    const allObjects = isRelationLinksValue(items);
    if (items.length > 0 && !allObjects) {
      if (items.every((item) => typeof item === 'string')) {
        refuse(property, 'relation_not_a_list', member);
      }
      refuseInvalidLink(property, member);
    }
    const links: RelationLink[] = [];
    for (const item of items) {
      const parsed = parseRelationLink(item);
      if (!parsed) refuseInvalidLink(property, member);
      links.push(parsed);
    }
    const ids = links.map((link) => link.card);
    const had = new Set(relationCardIds(previous[property]));
    if (ids.length > RELATION_LIMIT && ids.some((id) => !had.has(id))) {
      refuse(property, 'relation_too_many', member);
    }
    if (ids.includes(card.id)) refuse(property, 'relation_to_itself', member);
    if (new Set(ids).size !== ids.length) refuse(property, 'relation_repeated', member);
    for (const id of ids.filter((each) => !had.has(each))) {
      const type = typeOnBoard(world, card.containerId, id);
      if (type === undefined) refuse(property, 'relation_card_not_found', member);
      if (type !== 'card') refuse(property, 'relation_not_a_card', member);
    }
    normalized.push([property, links]);
  }
  for (const [property, links] of normalized) {
    values[property] = links;
  }
}

/**
 * Renames, in a block copied from the board `source`, what it names of the
 * blocks copied with it (`renamed`, old identifier to new): a view's
 * `cardOrder` and `contentOrder` and `defaultTemplateId`, as the server does
 * (anything else keeps naming the source), what a body names (what was not
 * copied it names as before), and what a card's relations name (preserving
 * `kind` and `lagDays`).
 * A relation names the copies of the copied cards (`cardIds`) and nothing
 * else: what is not a copied card, the card itself and a repeat go, a value
 * that is no list becomes null, and at most RELATION_LIMIT stay.
 */
export function renameCopied(
  source: Container,
  block: StoredBlock,
  renamed: ReadonlyMap<string, string>,
  cardIds: ReadonlySet<string>,
): void {
  const rename = (id: string): string => renamed.get(id) ?? id;
  const fields = block.fields;
  for (const name of ['cardOrder', 'contentOrder']) {
    const value = fields[name];
    if (Array.isArray(value)) {
      fields[name] = (value as unknown[]).map((item) =>
        typeof item === 'string' ? rename(item) : item,
      );
    }
  }
  if (typeof fields.defaultTemplateId === 'string') {
    fields.defaultTemplateId = rename(fields.defaultTemplateId);
  }
  if (block.body) block.body = renameNamed(block.body, rename);
  if (block.type !== 'card') return;
  const values = propertiesOf(block);
  const relIds = new Set(relationIds(source));
  for (const [property, value] of Object.entries(values)) {
    if (!relIds.has(property) && isRelationLinksValue(value)) {
      values[property] = null;
    }
  }
  for (const property of relIds) {
    const value = values[property];
    if (value === undefined) continue;
    if (!Array.isArray(value)) {
      values[property] = null;
      continue;
    }
    const kept = parseRelationValue(value)
      .filter((link) => link.card !== block.id && cardIds.has(link.card))
      .slice(0, RELATION_LIMIT)
      .map((link) => ({ ...link, card: rename(link.card) }));
    values[property] = kept;
  }
}
