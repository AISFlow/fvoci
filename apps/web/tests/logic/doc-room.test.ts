/**
 * One room of a body as its client sees it (`logic/doc-room.ts`): what it
 * answers to each thing the server says, the four times the document is
 * received again, and how changes are gathered into frames.
 */
import { describe, expect, it } from 'vitest';
import * as Y from 'yjs';

import {
  DocRoom,
  FLUSH_WAIT,
  PACE_WAIT,
  RESET_WAIT,
  type RoomEffect,
  type RoomEvent,
  type RoomFrame,
} from '$lib/logic/doc-room.ts';

const WHOLE = new Uint8Array([0, 0]);
const STEP_2 = 1;
const UPDATE = 2;

/** An update of `size` bytes of text, made on a document of its own. */
function update(size = 10): Uint8Array {
  const doc = new Y.Doc();
  doc.getText('t').insert(0, 'x'.repeat(size));
  return Y.encodeStateAsUpdate(doc);
}

class Harness {
  readonly room = new DocRoom((updates) => Y.mergeUpdates(updates), { updateBytes: 1024 });
  effects: RoomEffect[] = [];

  take(event: RoomEvent): RoomEffect[] {
    this.effects = this.room.take(event);
    return this.effects;
  }

  of<T extends RoomEffect['type']>(type: T): Extract<RoomEffect, { type: T }>[] {
    return this.effects.filter(
      (effect): effect is Extract<RoomEffect, { type: T }> => effect.type === type,
    );
  }

  sent(): RoomFrame[] {
    return this.of('send').map((effect) => effect.frame);
  }

  /** Up, joined as room `doc` of `epoch`, and the whole document received. */
  joined(doc = 1, epoch = 7, mode: 'write' | 'read' = 'write'): void {
    this.take({ type: 'up' });
    this.take({ type: 'message', message: { type: 'docJoined', doc, epoch, mode } });
    this.take({ type: 'frame', frame: { doc, epoch, sync: STEP_2, payload: WHOLE } });
  }

  /** A local change, gathered and sent. */
  write(payload = update()): RoomFrame[] {
    this.take({ type: 'local', update: payload });
    this.take({ type: 'timer', timer: 'flush' });
    return this.sent();
  }
}

describe('joining', () => {
  it('asks once the connection is there, and holds the server whole document first', () => {
    const room = new Harness();
    expect(room.take({ type: 'local', update: update() })).toEqual([]);
    expect(room.take({ type: 'up' }).map((effect) => effect.type)).toEqual([
      'cancel',
      'join',
      'timer',
      'changed',
    ]);
    expect(room.room.status.phase).toBe('joining');
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 4, epoch: 2, mode: 'write' },
    });
    expect(room.room.status.writable).toBe(false);
    // A frame of another room, then an update before the whole document: dropped.
    expect(
      room.take({ type: 'frame', frame: { doc: 5, epoch: 2, sync: STEP_2, payload: WHOLE } }),
    ).toEqual([]);
    expect(
      room.take({ type: 'frame', frame: { doc: 4, epoch: 2, sync: UPDATE, payload: WHOLE } }),
    ).toEqual([]);
    room.take({ type: 'frame', frame: { doc: 4, epoch: 2, sync: STEP_2, payload: WHOLE } });
    expect(room.of('fresh')).toEqual([{ type: 'fresh', update: WHOLE }]);
    expect(room.room.status).toMatchObject({
      phase: 'joined',
      mode: 'write',
      writable: true,
      held: true,
    });
  });

  it('is read only when let in to read, and a change made then is never sent', () => {
    const room = new Harness();
    room.joined(1, 1, 'read');
    expect(room.room.status.writable).toBe(false);
    expect(room.write()).toEqual([]);
    expect(room.room.status.pending).toBe(false);
  });

  it('is refused for good when the card is not there for the reader, and its document let go', () => {
    const room = new Harness();
    room.joined();
    room.take({ type: 'message', message: { type: 'docLeft', doc: 1, reason: 'not_found' } });
    expect(room.room.status).toMatchObject({
      phase: 'out',
      reason: 'not_found',
      retrying: false,
      held: false,
    });
    expect(room.of('drop')).toHaveLength(1);
  });

  it('tries again, later each time, what is refused for now', () => {
    const room = new Harness();
    room.take({ type: 'up' });
    const waits: number[] = [];
    for (let attempt = 0; attempt < 3; attempt += 1) {
      room.take({ type: 'message', message: { type: 'docRefused', reason: 'document_full' } });
      const timer = room.of('timer')[0];
      waits.push(timer?.wait ?? 0);
      expect(room.room.status).toMatchObject({ phase: 'out', retrying: true });
      expect(
        room.take({ type: 'timer', timer: 'retry' }).some((effect) => effect.type === 'join'),
      ).toBe(true);
    }
    expect(waits).toEqual([1000, 2000, 5000]);
  });
});

describe('the frames of a room', () => {
  it('drops those of an old epoch and of a room not known', () => {
    const room = new Harness();
    room.joined(1, 7);
    expect(
      room.take({ type: 'frame', frame: { doc: 1, epoch: 6, sync: UPDATE, payload: WHOLE } }),
    ).toEqual([]);
    expect(
      room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: UPDATE, payload: WHOLE } }),
    ).toEqual([]);
    expect(
      room.take({ type: 'frame', frame: { doc: 1, epoch: 7, sync: UPDATE, payload: WHOLE } }),
    ).toEqual([{ type: 'apply', update: WHOLE }]);
    // Answers of a room left: dropped.
    expect(
      room.take({
        type: 'message',
        message: { type: 'docNack', doc: 2, epoch: 7, reason: 'update_refused' },
      }),
    ).toEqual([]);
  });
});

describe('gathering changes', () => {
  it('sends the changes of one wait as one frame, and the next frame after a pause', () => {
    const room = new Harness();
    room.joined();
    const first = room.take({ type: 'local', update: update(3) });
    expect(first).toContainEqual({ type: 'timer', timer: 'flush', wait: FLUSH_WAIT });
    expect(room.take({ type: 'local', update: update(4) })).toEqual([]);
    room.take({ type: 'timer', timer: 'flush' });
    expect(room.sent()).toHaveLength(1);
    expect(room.room.status.pending).toBe(true);
    room.take({ type: 'message', message: { type: 'docAck', doc: 1, epoch: 7 } });
    expect(room.room.status.pending).toBe(false);
  });

  it('cuts what is over the limit into frames under it, sent one a wait', () => {
    const room = new Harness();
    room.joined();
    for (let index = 0; index < 6; index += 1) room.take({ type: 'local', update: update(300) });
    room.take({ type: 'timer', timer: 'flush' });
    const frames = [...room.sent()];
    expect(frames).toHaveLength(1);
    expect(room.of('timer')).toEqual([{ type: 'timer', timer: 'flush', wait: FLUSH_WAIT }]);
    while (room.of('timer').length > 0) {
      room.take({ type: 'timer', timer: 'flush' });
      frames.push(...room.sent());
    }
    expect(frames.length).toBeGreaterThan(1);
    for (const frame of frames) expect(frame.payload.length).toBeLessThanOrEqual(1024);
  });

  it('cannot keep one change larger than the server takes: the document is received again and the loss said', () => {
    const room = new Harness();
    room.joined();
    room.take({ type: 'local', update: update(2000) });
    room.take({ type: 'timer', timer: 'flush' });
    expect(room.sent()).toEqual([]);
    expect(room.of('join')).toHaveLength(1);
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
    });
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('discarded')).toEqual([{ type: 'discarded', why: 'update_too_large' }]);
    expect(room.of('fresh')).toHaveLength(1);
  });

  it('sends everything waiting at once when told to settle', () => {
    const room = new Harness();
    room.joined();
    for (let index = 0; index < 6; index += 1) room.take({ type: 'local', update: update(300) });
    room.take({ type: 'settle' });
    expect(room.sent().length).toBeGreaterThan(1);
  });
});

describe('receiving the document again', () => {
  it('docReset of a new epoch: the document is another, and changes not taken are said lost', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.write();
    room.take({ type: 'message', message: { type: 'docReset', doc: 1, epoch: 8 } });
    expect(room.room.status).toMatchObject({ phase: 'receiving', writable: false });
    // The late answer of the change sent in epoch 7 is about the document dropped.
    expect(
      room.take({
        type: 'message',
        message: { type: 'docNack', doc: 1, epoch: 7, reason: 'update_refused' },
      }),
    ).toEqual([]);
    expect(
      room.take({ type: 'frame', frame: { doc: 1, epoch: 7, sync: UPDATE, payload: WHOLE } }),
    ).toEqual([]);
    room.take({ type: 'frame', frame: { doc: 1, epoch: 8, sync: STEP_2, payload: WHOLE } });
    expect(room.of('discarded')).toEqual([{ type: 'discarded', why: 'document_reset' }]);
    expect(room.of('fresh')).toHaveLength(1);
    expect(room.room.status).toMatchObject({ phase: 'joined', pending: false, writable: true });
  });

  it('docReset of the same epoch: the whole is merged into the document held, nothing lost', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.write();
    room.take({ type: 'message', message: { type: 'docReset', doc: 1, epoch: 7 } });
    room.take({ type: 'frame', frame: { doc: 1, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.effects.map((effect) => effect.type)).toEqual(['apply']);
    room.take({ type: 'message', message: { type: 'docAck', doc: 1, epoch: 7 } });
    expect(room.room.status.pending).toBe(false);
  });

  it('docNack: joins once more, drops the document for the server, and says the change is lost', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.write();
    room.write();
    room.take({
      type: 'message',
      message: { type: 'docNack', doc: 1, epoch: 7, reason: 'update_refused' },
    });
    expect(room.of('join')).toHaveLength(1);
    // The second change's answer comes for a room left: nothing more.
    expect(
      room.take({
        type: 'message',
        message: { type: 'docNack', doc: 1, epoch: 7, reason: 'update_refused' },
      }),
    ).toEqual([]);
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
    });
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('discarded')).toEqual([{ type: 'discarded', why: 'update_refused' }]);
    expect(room.of('fresh')).toHaveLength(1);
  });

  it('docNack out of step: waits for the docReset that explains it, then joins if none came', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.write();
    room.take({
      type: 'message',
      message: { type: 'docNack', doc: 1, epoch: 7, reason: 'update_out_of_step' },
    });
    expect(room.of('timer')).toEqual([{ type: 'timer', timer: 'reset', wait: RESET_WAIT }]);
    expect(room.room.status.writable).toBe(false);
    room.take({ type: 'timer', timer: 'reset' });
    expect(room.of('join')).toHaveLength(1);
  });

  it('docNack document unavailable: the change is kept and offered again once let in', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.write();
    room.take({
      type: 'message',
      message: { type: 'docNack', doc: 1, epoch: 7, reason: 'document_unavailable' },
    });
    expect(room.of('timer')).toEqual([{ type: 'timer', timer: 'retry', wait: PACE_WAIT }]);
    room.take({ type: 'timer', timer: 'retry' });
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
    });
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('discarded')).toEqual([]);
    expect(room.of('offer')).toEqual([{ type: 'offer', have: WHOLE }]);
    room.take({ type: 'missing', update: update() });
    room.take({ type: 'timer', timer: 'flush' });
    expect(room.sent()).toHaveLength(1);
  });

  it.each([['too_many_frames'], ['connection_too_slow']])(
    'docLeft for %s: joins again after a pause, going on with the document held',
    (reason) => {
      const room = new Harness();
      room.joined(1, 7);
      room.take({ type: 'message', message: { type: 'docLeft', doc: 1, reason } });
      expect(room.room.status.phase).toBe('receiving');
      expect(room.of('timer')).toEqual([{ type: 'timer', timer: 'retry', wait: PACE_WAIT }]);
      room.take({ type: 'timer', timer: 'retry' });
      room.take({
        type: 'message',
        message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
      });
      room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
      expect(room.effects.map((effect) => effect.type)).toEqual(['apply', 'changed']);
    },
  );

  it.each([['document_unavailable'], ['too_many_documents'], ['internal']])(
    'docLeft for %s: out, and tried again by itself',
    (reason) => {
      const room = new Harness();
      room.joined(1, 7);
      room.take({ type: 'message', message: { type: 'docLeft', doc: 1, reason } });
      expect(room.room.status).toMatchObject({ phase: 'out', reason, retrying: true, held: true });
      room.take({ type: 'timer', timer: 'retry' });
      expect(room.of('join')).toHaveLength(1);
    },
  );

  it('a dropped connection: read only until it is back, then joined again', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.take({ type: 'down' });
    expect(room.room.status).toMatchObject({ phase: 'offline', writable: false, held: true });
    // Nothing can be sent while there is no connection.
    expect(room.write()).toEqual([]);
    room.take({ type: 'up' });
    expect(room.of('join')).toHaveLength(1);
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
    });
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    // The document held goes on; what was written meanwhile is offered.
    expect(room.of('offer')).toHaveLength(1);
  });

  it('a connection that came back names the document held, and answers the state vector of the server with what it lacks', () => {
    const STEP_1 = 0;
    const theirs = new Uint8Array([1, 9, 3]);
    const room = new Harness();
    room.joined(1, 7);
    room.take({ type: 'down' });
    room.write();
    // The join says which document is held: the server may send what it lacks, not the whole.
    expect(room.take({ type: 'up' })).toContainEqual({ type: 'join', held: 7 });
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
    });
    // The server's state vector first: nothing is done with it yet.
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_1, payload: theirs } });
    expect(room.effects).toEqual([]);
    expect(room.room.status.phase).toBe('receiving');
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('apply')).toEqual([{ type: 'apply', update: WHOLE }]);
    expect(room.of('answer')).toEqual([{ type: 'answer', theirs }]);
    expect(room.of('offer')).toEqual([]);
    // What the server lacks goes to it as a change, and is answered.
    const lacked = update();
    room.take({ type: 'missing', update: lacked });
    room.take({ type: 'timer', timer: 'flush' });
    expect(room.sent()).toEqual([{ doc: 2, epoch: 7, sync: UPDATE, payload: lacked }]);
    expect(room.room.status.pending).toBe(true);
    room.take({ type: 'message', message: { type: 'docAck', doc: 2, epoch: 7 } });
    expect(room.room.status.pending).toBe(false);

    // Nothing written meanwhile: the state vector is not answered.
    room.take({ type: 'down' });
    room.take({ type: 'up' });
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 3, epoch: 7, mode: 'write' },
    });
    room.take({ type: 'frame', frame: { doc: 3, epoch: 7, sync: STEP_1, payload: theirs } });
    room.take({ type: 'frame', frame: { doc: 3, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('answer')).toEqual([]);
    expect(room.room.status).toMatchObject({ phase: 'joined', writable: true, pending: false });
  });

  it('forgets the state vector of the server when the connection drops, or the document is sent anew, before what follows it', () => {
    const STEP_1 = 0;
    const theirs = new Uint8Array([1, 9, 3]);
    const back = (room: Harness, doc: number): void => {
      room.take({ type: 'down' });
      room.take({ type: 'up' });
      room.take({
        type: 'message',
        message: { type: 'docJoined', doc, epoch: 7, mode: 'write' },
      });
    };
    // Dropped between the two: the next room's whole is answered by the whole, not by a
    // state vector of the room before.
    const room = new Harness();
    room.joined(1, 7);
    room.take({ type: 'down' });
    room.write();
    room.take({ type: 'up' });
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
    });
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_1, payload: theirs } });
    back(room, 3);
    room.take({ type: 'frame', frame: { doc: 3, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('answer')).toEqual([]);
    expect(room.of('offer')).toEqual([{ type: 'offer', have: WHOLE }]);

    // The document made anew between the two: the whole of the new one, and the loss said.
    const reset = new Harness();
    reset.joined(1, 7);
    reset.take({ type: 'down' });
    reset.write();
    reset.take({ type: 'up' });
    reset.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
    });
    reset.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_1, payload: theirs } });
    reset.take({ type: 'message', message: { type: 'docReset', doc: 2, epoch: 8 } });
    reset.take({ type: 'frame', frame: { doc: 2, epoch: 8, sync: STEP_2, payload: WHOLE } });
    expect(reset.of('fresh')).toHaveLength(1);
    expect(reset.of('answer')).toEqual([]);
    expect(reset.of('discarded')).toEqual([{ type: 'discarded', why: 'document_reset' }]);
  });

  it('let in to read after being away with changes: the whole says whether the server lacks them', () => {
    // The server sends who reads the whole, though they named their document.
    const room = new Harness();
    room.joined(1, 7);
    room.take({ type: 'down' });
    room.write();
    room.take({ type: 'up' });
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'read' },
    });
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('offer')).toEqual([{ type: 'offer', have: WHOLE }]);
    // The server had them all (only its answer was lost): nothing is said lost.
    const had = room.take({ type: 'missing', update: undefined });
    expect(had.some((effect) => effect.type === 'discarded')).toBe(false);
    expect(room.room.status).toMatchObject({ phase: 'joined', mode: 'read', pending: false });

    // The server lacks some: they cannot go any more, and that is said.
    const lacks = new Harness();
    lacks.joined(1, 7);
    lacks.take({ type: 'down' });
    lacks.write();
    lacks.take({ type: 'up' });
    lacks.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'read' },
    });
    lacks.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    lacks.take({ type: 'missing', update: update() });
    expect(lacks.of('join')).toEqual([{ type: 'join', held: undefined }]);
  });

  it('names no document when the one held is to be dropped, and takes a state vector of another epoch for nothing', () => {
    const STEP_1 = 0;
    const room = new Harness();
    room.joined(1, 7);
    room.write();
    // A change refused: the document is the reader's alone, and is not named.
    const refused = room.take({
      type: 'message',
      message: { type: 'docNack', doc: 1, epoch: 7, reason: 'update_refused' },
    });
    expect(refused).toContainEqual({ type: 'join', held: undefined });
    // Holding none, none is named.
    expect(new Harness().take({ type: 'up' })).toContainEqual({ type: 'join', held: undefined });

    // Made anew meanwhile: a state vector of the new epoch is no word about the one held.
    const away = new Harness();
    away.joined(1, 7);
    away.take({ type: 'down' });
    away.write();
    away.take({ type: 'up' });
    away.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 9, mode: 'write' },
    });
    away.take({
      type: 'frame',
      frame: { doc: 2, epoch: 9, sync: STEP_1, payload: new Uint8Array([0]) },
    });
    // What follows a state vector is a difference, and one for another document than the one
    // held is no document: it is not taken for the whole, which is asked for, naming nothing.
    away.take({ type: 'frame', frame: { doc: 2, epoch: 9, sync: STEP_2, payload: WHOLE } });
    expect(away.of('fresh')).toEqual([]);
    expect(away.of('answer')).toEqual([]);
    expect(away.of('join')).toEqual([{ type: 'join', held: undefined }]);
    away.take({
      type: 'message',
      message: { type: 'docJoined', doc: 3, epoch: 9, mode: 'write' },
    });
    away.take({ type: 'frame', frame: { doc: 3, epoch: 9, sync: STEP_2, payload: WHOLE } });
    expect(away.of('fresh')).toHaveLength(1);
    expect(away.of('discarded')).toHaveLength(1);

    // The same for a document given up after it was named (a change too large to send, found
    // while an older join's answer was still on its way): the difference is not the document.
    const gave = new Harness();
    gave.joined(1, 7);
    gave.take({ type: 'down' });
    gave.write();
    gave.take({ type: 'up' });
    gave.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
    });
    gave.take({
      type: 'frame',
      frame: { doc: 2, epoch: 7, sync: STEP_1, payload: new Uint8Array([0]) },
    });
    gave.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    // What the server lacks is larger than it takes: the document is given up, joined anew.
    gave.take({ type: 'missing', update: update(2000) });
    expect(gave.take({ type: 'timer', timer: 'flush' })).toContainEqual({
      type: 'join',
      held: undefined,
    });
    // The answer of a join that named the document comes first: not taken for the whole.
    gave.take({
      type: 'message',
      message: { type: 'docJoined', doc: 3, epoch: 7, mode: 'write' },
    });
    gave.take({
      type: 'frame',
      frame: { doc: 3, epoch: 7, sync: STEP_1, payload: new Uint8Array([0]) },
    });
    gave.take({ type: 'frame', frame: { doc: 3, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(gave.of('fresh')).toEqual([]);
    expect(gave.of('apply')).toEqual([]);
    expect(gave.room.status.phase).not.toBe('joined');
  });

  it('a dropped connection with the document made anew meanwhile: the change written is said lost', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.write();
    room.take({ type: 'down' });
    room.take({ type: 'up' });
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 9, mode: 'write' },
    });
    room.take({ type: 'frame', frame: { doc: 2, epoch: 9, sync: STEP_2, payload: WHOLE } });
    expect(room.of('discarded')).toEqual([{ type: 'discarded', why: 'document_reset' }]);
    expect(room.of('fresh')).toHaveLength(1);
  });
});

describe('the way of writing', () => {
  it('docModeChanged read stops the sending; a change made meanwhile is the document no longer being the server, joined again on write', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.take({ type: 'message', message: { type: 'docModeChanged', doc: 1, mode: 'read' } });
    expect(room.room.status.writable).toBe(false);
    room.take({ type: 'local', update: update() });
    room.take({ type: 'message', message: { type: 'docModeChanged', doc: 1, mode: 'write' } });
    expect(room.of('join')).toHaveLength(1);
  });

  it('a change refused as read only turns the room to reading and receives the document again', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.write();
    room.take({
      type: 'message',
      message: { type: 'docNack', doc: 1, epoch: 7, reason: 'document_read_only' },
    });
    expect(room.room.status.mode).toBe('read');
    expect(room.of('join')).toHaveLength(1);
  });
});

describe('answers that come late, and joins that cross', () => {
  it('a change gathered when the room turns to reading is said lost, and the document received again', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.take({ type: 'local', update: update() });
    room.take({ type: 'message', message: { type: 'docModeChanged', doc: 1, mode: 'read' } });
    expect(room.of('join')).toHaveLength(1);
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'read' },
    });
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('discarded')).toEqual([{ type: 'discarded', why: 'document_read_only' }]);
    expect(room.room.status).toMatchObject({ mode: 'read', pending: false, writable: false });
  });

  it('asks to be let in once while a join is on its way, and takes a join with no answer as refused for now', () => {
    const room = new Harness();
    room.take({ type: 'up' });
    expect(room.take({ type: 'retry' }).some((effect) => effect.type === 'join')).toBe(false);
    room.take({ type: 'timer', timer: 'answer' });
    expect(room.room.status).toMatchObject({
      phase: 'out',
      reason: 'document_unavailable',
      retrying: true,
    });
    expect(room.take({ type: 'retry' }).some((effect) => effect.type === 'join')).toBe(true);
  });

  it('sends what was written before a change too large, then receives the document again', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.take({ type: 'local', update: update(10) });
    room.take({ type: 'local', update: update(2000) });
    room.take({ type: 'timer', timer: 'flush' });
    expect(room.sent()).toHaveLength(1);
    expect(room.of('join')).toHaveLength(1);
  });

  it('does not count the late answer of a change given up as the answer of the one sent after it', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.take({ type: 'local', update: update() });
    room.take({ type: 'settle' });
    room.take({ type: 'local', update: update() });
    room.take({ type: 'settle' });
    room.take({
      type: 'message',
      message: { type: 'docNack', doc: 1, epoch: 7, reason: 'update_out_of_step' },
    });
    room.take({ type: 'message', message: { type: 'docReset', doc: 1, epoch: 7 } });
    room.take({ type: 'frame', frame: { doc: 1, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('offer')).toHaveLength(1);
    room.take({ type: 'missing', update: update() });
    room.take({ type: 'settle' });
    expect(room.sent()).toHaveLength(1);
    // The answer of the second change, sent before the refusal, comes now.
    room.take({ type: 'message', message: { type: 'docAck', doc: 1, epoch: 7 } });
    expect(room.room.status.pending).toBe(true);
    room.take({ type: 'message', message: { type: 'docAck', doc: 1, epoch: 7 } });
    expect(room.room.status.pending).toBe(false);
  });

  it('offers nothing when the document held has nothing the server lacks', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.write();
    room.take({ type: 'down' });
    room.take({ type: 'up' });
    room.take({
      type: 'message',
      message: { type: 'docJoined', doc: 2, epoch: 7, mode: 'write' },
    });
    room.take({ type: 'frame', frame: { doc: 2, epoch: 7, sync: STEP_2, payload: WHOLE } });
    expect(room.of('offer')).toHaveLength(1);
    room.take({ type: 'missing', update: undefined });
    expect(room.room.status.pending).toBe(false);
    room.take({ type: 'timer', timer: 'flush' });
    expect(room.sent()).toEqual([]);
  });

  it('stays out after not_found when the connection comes back', () => {
    const room = new Harness();
    room.joined(1, 7);
    room.take({ type: 'message', message: { type: 'docLeft', doc: 1, reason: 'not_found' } });
    room.take({ type: 'down' });
    expect(room.take({ type: 'up' }).some((effect) => effect.type === 'join')).toBe(false);
  });
});

describe('the carets of a room', () => {
  const CARET = new Uint8Array([1, 7, 1, 4, 110, 117, 108, 108]);

  it("takes in the others' of the document held, in the room it is in", () => {
    const room = new Harness();
    room.joined(4, 7);
    room.take({ type: 'carets', carets: { doc: 4, epoch: 7, update: CARET } });
    expect(room.of('carets')).toEqual([{ type: 'carets', update: CARET }]);
    // Of another room, and of another making of the document.
    room.take({ type: 'carets', carets: { doc: 5, epoch: 7, update: CARET } });
    expect(room.of('carets')).toEqual([]);
    room.take({ type: 'carets', carets: { doc: 4, epoch: 6, update: CARET } });
    expect(room.of('carets')).toEqual([]);
    // While another document is on its way.
    room.take({ type: 'message', message: { type: 'docReset', doc: 4, epoch: 8 } });
    room.take({ type: 'carets', carets: { doc: 4, epoch: 7, update: CARET } });
    room.take({ type: 'carets', carets: { doc: 4, epoch: 8, update: CARET } });
    expect(room.of('carets')).toEqual([]);
  });

  it("tells the reader's own while they write in the room, and never while they only read", () => {
    const room = new Harness();
    // Not in a room yet.
    room.take({ type: 'caret', update: CARET });
    expect(room.of('tell')).toEqual([]);
    room.joined(4, 7);
    room.take({ type: 'caret', update: CARET });
    expect(room.of('tell')).toEqual([
      { type: 'tell', carets: { doc: 4, epoch: 7, update: CARET } },
    ]);
    room.take({ type: 'message', message: { type: 'docModeChanged', doc: 4, mode: 'read' } });
    room.take({ type: 'caret', update: CARET });
    expect(room.of('tell')).toEqual([]);
    const reader = new Harness();
    reader.joined(1, 7, 'read');
    reader.take({ type: 'caret', update: CARET });
    expect(reader.of('tell')).toEqual([]);
    // What a reader is told of the others they take in.
    reader.take({ type: 'carets', carets: { doc: 1, epoch: 7, update: CARET } });
    expect(reader.of('carets')).toHaveLength(1);
  });

  it("lets go of the others' when the room is left: put out, joined again, the connection gone", () => {
    const room = new Harness();
    room.joined(4, 7);
    room.take({ type: 'message', message: { type: 'docLeft', doc: 4, reason: 'not_found' } });
    expect(room.of('unseen')).toHaveLength(1);
    const dropped = new Harness();
    dropped.joined(4, 7);
    dropped.take({ type: 'down' });
    expect(dropped.of('unseen')).toHaveLength(1);
    // Not in a room: nothing to let go of.
    dropped.take({ type: 'down' });
    expect(dropped.of('unseen')).toEqual([]);
    dropped.take({ type: 'caret', update: CARET });
    expect(dropped.of('tell')).toEqual([]);
  });
});
