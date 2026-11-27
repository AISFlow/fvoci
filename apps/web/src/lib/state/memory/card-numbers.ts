/**
 * The numbers of the cards of the world in memory, as the server gives them
 * (`crates/store/src/card_numbers.rs`): each board counts its cards from 1,
 * one more for each card made there, and never gives a number again, also
 * once the card is gone. A template of the board's cards has none. A copy and
 * the card of an archive are new cards with new numbers; a card that comes
 * back from the trash has its own, which it took there.
 */
import { oldestFirst, type StoredBlock, type World } from './world.ts';

/** Whether the block has a number: a card that is no template. */
function takesNumber(block: StoredBlock): boolean {
  return block.type === 'card' && block.fields.isTemplate !== true;
}

/** Gives a block being stored the next number of its board if it takes one, and none if not. */
export function numberBlock(world: World, block: StoredBlock): void {
  delete block.number;
  if (!takesNumber(block)) return;
  const next = world.nextCardNumbers.get(block.containerId) ?? 1;
  world.nextCardNumbers.set(block.containerId, next + 1);
  block.number = next;
}

/**
 * The world with the cards it holds numbered as migration 58 numbers the
 * cards there are: on each board from 1, in the order they were made, those
 * made at once by their identifiers.
 */
export function numberedWorld(world: World): World {
  const cards = [...world.blocks.values()].filter(takesNumber).sort(oldestFirst);
  for (const card of cards) numberBlock(world, card);
  return world;
}
