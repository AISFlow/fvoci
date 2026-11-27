/**
 * A card's body opened before the code that holds its document has
 * arrived. Both backends answer `openDoc` at once with this; the room's
 * client (`doc-client.ts`, and Yjs with it) is fetched behind it, so that
 * neither is part of what the first screen loads.
 */
import { noteChunkFailure } from '../chunk-reload.svelte.ts';
import type { RoomLine } from '../logic/doc-room-types.ts';

import type { BodyDoc, BodyDocHears, BodyDocState } from './backend.ts';

const WAITING: BodyDocState = {
  status: 'joining',
  doc: undefined,
  carets: undefined,
  composing: () => undefined,
  readsFirst: () => () => undefined,
  mode: 'read',
  reason: undefined,
  writable: false,
  pending: false,
  retrying: false,
};

/** What `load` could not fetch (the connection dropped, a new version was deployed). */
const NOT_LOADED = 'not_loaded';

const NOT_LOADED_STATE: BodyDocState = { ...WAITING, status: 'out', reason: NOT_LOADED };

/** `load` fetches what opens the body; until it has, the body is being joined. */
export function openLater(
  load: () => Promise<(hears: BodyDocHears) => BodyDoc>,
  hears: BodyDocHears,
): BodyDoc {
  let opened: BodyDoc | undefined;
  let closed = false;
  let failed = false;
  // Asked as a function: `closed` can change while `open` runs, which a reading of it does not show.
  const isClosed = (): boolean => closed;
  const fetch = (): void => {
    failed = false;
    load().then(
      (open) => {
        if (closed) return;
        const made = open(hears);
        // Closed by what it told while it was being made.
        if (isClosed()) made.close();
        else {
          opened = made;
          hears.state(made.state);
        }
      },
      () => {
        void noteChunkFailure();
        if (closed) return;
        failed = true;
        hears.state(NOT_LOADED_STATE);
      },
    );
  };
  fetch();
  return {
    get state() {
      if (opened) return opened.state;
      return failed ? NOT_LOADED_STATE : WAITING;
    },
    retry: () => {
      if (opened) opened.retry();
      else if (failed) {
        fetch();
        hears.state(WAITING);
      }
    },
    settle: () => opened?.settle() ?? Promise.resolve(),
    close: () => {
      closed = true;
      opened?.close();
    },
  };
}

/** Opens a body on a way into its room, the room's client fetched first. */
export function openRoomLater(line: RoomLine, hears: BodyDocHears): BodyDoc {
  return openLater(
    () => import('./doc-client.ts').then((client) => (told) => client.openRoom(line, told)),
    hears,
  );
}
