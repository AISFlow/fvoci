/**
 * The limits and the waits of a room of a card's body as its client keeps
 * them (`doc-room.ts`), and the reasons of the server it tells apart.
 */

export interface RoomLimits {
  /** The largest update the server takes, in bytes (`collab_update_bytes`). */
  updateBytes: number;
}

export const ROOM_LIMITS: RoomLimits = { updateBytes: 64 * 1024 };

/**
 * How long changes are gathered into one frame, and the pause between two
 * frames, in milliseconds: at most 33 frames a second where the server
 * allows 50.
 */
export const FLUSH_WAIT = 30;
/** How long a refusal as out of step waits for the `docReset` that explains it. */
export const RESET_WAIT = 1500;
/** The wait before joining again after being put out for sending or reading at the wrong pace. */
export const PACE_WAIT = 1000;
/** The waits before trying again what was refused for now, by tries; the last repeats. */
export const RETRY_WAITS = [1000, 2000, 5000, 15_000, 30_000];

/** How long a join waits for its answer before it is taken as refused for now. */
export const ANSWER_WAIT = 10_000;

/** A reason that does not pass by waiting: the card is not there for the reader. */
export const FINAL = new Set(['not_found']);
/** Being put out for these says nothing against the document: the room is joined again as it was left. */
export const PACE = new Set(['too_many_frames', 'connection_too_slow']);
