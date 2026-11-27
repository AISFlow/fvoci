/**
 * How the saving of an open card's body stands, for the line at the top of
 * the card. The body is written in its room (`CardStore.openBody`): what the
 * reader types is sent as they type, and it is saved once the server has it.
 *
 * - `idle`: nothing was written in this visit.
 * - `saving`: what was written is on its way.
 * - `saved`: the server has everything written in this visit.
 * - `unsaved`: what was written waits for the room (the connection dropped,
 *   or the reader is not in the room now); it is sent once let in again.
 */
import type { CardStore } from '$lib/state/index.ts';

export type SaveState = 'idle' | 'saving' | 'saved' | 'unsaved';

export function saveStateOf(card: CardStore): SaveState {
  const room = card.room;
  if (room?.pending && room.status !== 'joined') return 'unsaved';
  if (card.bodySaving) return 'saving';
  return card.bodyEdited ? 'saved' : 'idle';
}
