/**
 * Undo and redo asked for while writes are under way: a change is a step
 * from the moment it is made, undo and redo wait their turn, and a change
 * that is refused leaves nothing for them to do.
 */
import { describe, expect, it } from 'vitest';

import { UndoManager } from '$lib/logic/undo.ts';

/** A write held until the test answers it. */
interface Held {
  promise: Promise<void>;
  answer: () => void;
  refuse: () => void;
}

function held(): Held {
  let answer = (): void => undefined;
  let refuse = (): void => undefined;
  const promise = new Promise<void>((resolve, reject) => {
    answer = resolve;
    refuse = () => {
      reject(new Error('refused'));
    };
  });
  return { promise, answer, refuse };
}

/** A value changed through a backend whose every write the test holds. */
function setup() {
  const manager = new UndoManager();
  /** What the screen shows: changed at once, put back when a write is refused. */
  let shown = 'start';
  /** What the backend has stored. */
  let saved = 'start';
  const writes: Held[] = [];
  const write = async (value: string): Promise<void> => {
    const before = shown;
    shown = value;
    const one = held();
    writes.push(one);
    try {
      await one.promise;
      saved = value;
    } catch (error) {
      shown = before;
      throw error;
    }
  };
  /** Changes the value as a step of undo, from what is shown; a refusal is caught, as the stores do. */
  const change = async (to: string): Promise<void> => {
    const from = shown;
    await manager
      .perform(
        () => write(to),
        () => write(from),
      )
      .catch(() => undefined);
  };
  /** Lets every promise that can go on do so: a task runs after all of them. */
  const flush = (): Promise<void> =>
    new Promise((resolve) => {
      setTimeout(resolve);
    });
  return {
    manager,
    change,
    flush,
    writes,
    shown: () => shown,
    saved: () => saved,
    /** Answers the write at `at` among those sent and not answered yet. */
    answerAt: async (at: number) => {
      await flush();
      writes.splice(at, 1)[0]?.answer();
      await flush();
    },
    /** Answers the oldest write sent and not answered yet. */
    answer: async () => {
      await flush();
      writes.shift()?.answer();
      await flush();
    },
    refuse: async () => {
      await flush();
      writes.shift()?.refuse();
      await flush();
    },
  };
}

/** A change that adds `value` to `values`, as a step of `manager` (of a gesture, with `groupId`). */
function adding(manager: UndoManager, values: string[], value: string, groupId?: string) {
  return manager.perform(
    async () => {
      values.push(value);
    },
    async () => {
      values.splice(values.indexOf(value), 1);
    },
    value,
    groupId,
  );
}
describe('undo while a change is being written', () => {
  it('takes back that change once it is written, not the one before', async () => {
    const { manager, change, answer, shown, saved, writes } = setup();
    void change('one');
    await answer();
    void change('two');
    const undone = manager.undo();
    // Asked for at once: "one" is the next to undo, "two" the next to redo.
    expect(manager.canRedo).toBe(true);
    expect(manager.busy).toBe(true);
    expect(shown()).toBe('two');
    expect(writes).toHaveLength(1);
    await answer(); // "two" is written; the undo sends "one".
    expect(shown()).toBe('one');
    await answer();
    await undone;
    expect(saved()).toBe('one');
    expect(manager.busy).toBe(false);
    expect(manager.canUndo).toBe(true);
  });

  it('drops an undo that waited for a refused change, and the change before stays', async () => {
    const { manager, change, answer, refuse, shown, saved, writes } = setup();
    void change('one');
    await answer();
    void change('two');
    const undone = manager.undo();
    await refuse();
    await undone;
    expect(shown()).toBe('one');
    expect(saved()).toBe('one');
    // Nothing more was sent: "one" was not taken back.
    expect(writes).toHaveLength(0);
    expect(manager.canUndo).toBe(true);
    expect(manager.canRedo).toBe(false);
    expect(manager.busy).toBe(false);
  });

  it('undoes changes one after the other, each when its write is back, in any order of answers', async () => {
    const { manager, change, answerAt, shown, saved, writes } = setup();
    void change('one');
    void change('two');
    const first = manager.undo();
    const second = manager.undo();
    expect(manager.canUndo).toBe(false);
    // "two" is answered before "one": its undo goes out; the undo of "one" waits for "one".
    await answerAt(1);
    expect(writes).toHaveLength(2);
    await answerAt(1);
    expect(writes).toHaveLength(1);
    await answerAt(0);
    expect(writes).toHaveLength(1);
    await answerAt(0);
    await Promise.all([first, second]);
    expect(shown()).toBe('start');
    expect(saved()).toBe('start');
    expect(manager.canRedo).toBe(true);
  });

  it('keeps a second undo asked for meanwhile for the step before the refused one', async () => {
    const { manager, change, answer, refuse, shown, writes } = setup();
    void change('one');
    await answer();
    void change('two');
    const first = manager.undo();
    const second = manager.undo();
    await refuse();
    // The undo of "two" is dropped; the second was asked for "one", and takes it back.
    expect(writes).toHaveLength(1);
    await answer();
    await Promise.all([first, second]);
    expect(shown()).toBe('start');
    expect(manager.canUndo).toBe(false);
    expect(manager.canRedo).toBe(true);
  });

  it('drops an undo still waiting when a change is made: it was made on what the screen showed', async () => {
    const { manager, change, answer, shown, saved, writes } = setup();
    void change('one');
    await answer();
    void change('two');
    const undone = manager.undo();
    // Made at once, from "two", which is what is shown: the undo of "two" is dropped.
    void change('three');
    expect(shown()).toBe('three');
    expect(manager.canRedo).toBe(false);
    await answer(); // "two"
    await answer(); // "three"
    await undone;
    expect(manager.busy).toBe(false);
    expect(writes).toHaveLength(0);
    expect(saved()).toBe('three');
    // The steps are "one", "two" and "three", each undone in turn.
    const again = manager.undo();
    await answer();
    await again;
    expect(saved()).toBe('two');
    expect(manager.canUndo).toBe(true);
  });

  it('frees the queue from a write that never answers once a change is made', async () => {
    const { manager, change, answerAt, shown, writes } = setup();
    void change('one');
    const undone = manager.undo();
    expect(manager.busy).toBe(true);
    // "one" never answers. A change is made and sent at once all the same.
    void change('two');
    expect(writes).toHaveLength(2);
    await undone;
    expect(manager.busy).toBe(false);
    expect(shown()).toBe('two');
    // Its own undo is not held behind the write that hangs (the first one).
    const again = manager.undo();
    await answerAt(1); // "two"
    expect(writes).toHaveLength(2);
    await answerAt(1); // the undo of "two"
    await again;
    expect(shown()).toBe('one');
  });

  it('keeps a step whose undo failed after a new change in force, before that change', async () => {
    const { manager, change, answer, refuse, flush, writes } = setup();
    void change('one');
    await answer();
    const first = manager.undo().catch((error: unknown) => error);
    await flush(); // the undo of "one" is being written, and shows
    void change('two');
    const second = manager.undo(); // "two", asked for while the undo of "one" is written
    await refuse(); // the undo of "one"
    expect(await first).toEqual(new Error('refused'));
    // "one" is in force again, before "two"; the undo of "two" still holds.
    expect(manager.canUndo).toBe(true);
    expect(manager.canRedo).toBe(true);
    await answer(); // "two"; its undo is sent
    expect(writes).toHaveLength(1);
    await answer();
    await second;
    const third = manager.undo(); // "one"
    await answer();
    await third;
    expect(manager.canUndo).toBe(false);
    expect(manager.redoDescription).toBe(undefined);
    expect(manager.canRedo).toBe(true);
  });

  it('carries out one undo at a time, also after a change made meanwhile let the queue go', async () => {
    const { manager, change, answer, answerAt, refuse, flush, writes } = setup();
    void change('w');
    await answer();
    void change('x');
    await answer();
    const first = manager.undo().catch((error: unknown) => error); // "x"
    await flush(); // the undo of "x" is being written
    void change('c'); // takes the place of "x" in the history
    const second = manager.undo(); // "c"
    const third = manager.undo(); // "w"
    await answerAt(1); // "c"
    // The undo of "c" waits for the undo of "x" to be over: nothing more is sent.
    expect(writes).toHaveLength(1);
    await refuse(); // the undo of "x": "x" stays in force, before "c"
    expect(await first).toEqual(new Error('refused'));
    // The undo of "w" was asked for without knowing that "x" stays: both are dropped.
    await Promise.all([second, third]);
    expect(writes).toHaveLength(0);
    expect(manager.canUndo).toBe(true);
    expect(manager.canRedo).toBe(false);
  });

  it('forgets a step whose redo failed after a new change: what is in force is that change', async () => {
    const { manager, change, answer, refuse, flush } = setup();
    void change('one');
    await answer();
    await Promise.all([manager.undo(), answer()]);
    const redone = manager.redo().catch((error: unknown) => error);
    await flush(); // the redo of "one" is being written, and shows
    void change('two');
    await refuse(); // the redo of "one"
    expect(await redone).toEqual(new Error('refused'));
    await answer(); // "two"
    expect(manager.canUndo).toBe(true);
    expect(manager.canRedo).toBe(false);
    await Promise.all([manager.undo(), answer()]);
    expect(manager.canUndo).toBe(false);
  });

  it('keeps an undo that waits when a gesture changes nothing', async () => {
    const { manager, change, answer, saved } = setup();
    void change('one');
    const undone = manager.undo();
    await manager.group(async () => undefined);
    await answer(); // "one"
    await answer(); // its undo
    await undone;
    expect(saved()).toBe('start');
    expect(manager.canRedo).toBe(true);
  });

  it('keeps what can be redone when a change is refused, or a gesture changes nothing', async () => {
    const { manager, change, answer, refuse } = setup();
    void change('one');
    await answer();
    void change('two');
    await answer();
    await Promise.all([manager.undo(), answer()]);
    expect(manager.canRedo).toBe(true);
    await manager.group(async () => undefined);
    expect(manager.canRedo).toBe(true);
    void change('three');
    expect(manager.canRedo).toBe(false);
    await refuse();
    // "three" never happened: "two" can be done again.
    expect(manager.canRedo).toBe(true);
    expect(manager.canUndo).toBe(true);
  });
});

describe('redo while an undo is being written', () => {
  it('waits for the undo, then does the change again', async () => {
    const { manager, change, answer, shown, saved } = setup();
    void change('one');
    await answer();
    const undone = manager.undo();
    const redone = manager.redo();
    expect(manager.canRedo).toBe(false);
    await answer();
    expect(shown()).toBe('one');
    await answer();
    await Promise.all([undone, redone]);
    expect(saved()).toBe('one');
    expect(manager.canUndo).toBe(true);
  });

  it('is dropped when the undo is refused, and the change stays in force', async () => {
    const { manager, change, answer, refuse, shown, writes } = setup();
    void change('one');
    await answer();
    const undone = manager.undo().catch((error: unknown) => error);
    const redone = manager.redo();
    await refuse();
    expect(await undone).toEqual(new Error('refused'));
    await redone;
    expect(writes).toHaveLength(0);
    expect(shown()).toBe('one');
    expect(manager.canUndo).toBe(true);
    expect(manager.canRedo).toBe(false);
  });
});

describe('a gesture of several changes', () => {
  it('is one step from its start, and an undo asked for meanwhile waits for all of it', async () => {
    const manager = new UndoManager();
    const values: string[] = [];
    const gate = held();
    await adding(manager, values, 'before');
    const gesture = manager.group(async (groupId) => {
      await adding(manager, values, 'a', groupId);
      await gate.promise;
      await adding(manager, values, 'b', groupId);
    });
    const undone = manager.undo();
    gate.answer();
    await Promise.all([gesture, undone]);
    expect(values).toEqual(['before']);
    expect(manager.undoDescription).toBe('before');
    expect(manager.redoDescription).toBe('a');
  });

  it('drops undos asked for meanwhile when it goes on after a change made between its own', async () => {
    // A drop (the view, then the status) and a change of the status made while the drop waits.
    const manager = new UndoManager();
    let view = 'unmoved';
    let status = 'todo';
    const setting = (value: string, groupId?: string, write?: Promise<void>) => {
      const was = status;
      return manager.perform(
        async () => {
          status = value;
          await write;
        },
        async () => {
          status = was;
        },
        `status ${value}`,
        groupId,
      );
    };
    const viewWrite = held();
    const statusWrite = held();
    const gesture = manager.group(async (groupId) => {
      await manager.perform(
        async () => {
          view = 'moved';
          await viewWrite.promise;
        },
        async () => {
          view = 'unmoved';
        },
        'move',
        groupId,
      );
      await setting('done', groupId);
    });
    const changing = setting('doing', undefined, statusWrite.promise);
    // Asked of the change of the person, then of the drop.
    const first = manager.undo();
    const second = manager.undo();
    viewWrite.answer();
    await gesture;
    statusWrite.answer();
    await Promise.all([changing, first, second]);
    // The rest of the drop came after them: they were dropped, and nothing was written over it.
    expect([view, status]).toEqual(['moved', 'done']);
    expect(manager.undoDescription).toBe('status done');

    const undone: (string | undefined)[] = [];
    while (manager.canUndo) {
      undone.push(manager.undoDescription);
      await manager.undo();
    }
    expect(undone).toEqual(['status done', 'status doing', 'move']);
    expect([view, status]).toEqual(['unmoved', 'todo']);
  });

  it('goes on in a step after a change made between its own: undo goes in the order they were made', async () => {
    const manager = new UndoManager();
    const values: string[] = [];
    const gate = held();
    await adding(manager, values, 'before');
    const gesture = manager.group(async (groupId) => {
      await adding(manager, values, 'a', groupId);
      await gate.promise;
      await adding(manager, values, 'b', groupId);
    });
    await adding(manager, values, 'between');
    gate.answer();
    await gesture;
    expect(values).toEqual(['before', 'a', 'between', 'b']);
    const undone: (string | undefined)[] = [];
    for (let times = 0; times < 3; times += 1) {
      undone.push(manager.undoDescription);
      await manager.undo();
    }
    expect(undone).toEqual(['b', 'between', 'a']);
    expect(values).toEqual(['before']);
  });

  it('leaves no step when it changed nothing', async () => {
    const manager = new UndoManager();
    await manager.perform(
      async () => undefined,
      async () => undefined,
      'before',
    );
    await manager.group(async () => undefined);
    expect(manager.undoDescription).toBe('before');
  });
});

describe('a history cleared while it is under way (another workspace entered)', () => {
  it('does not remember a change under way, nor take back an undo under way', async () => {
    const { manager, change, answer, flush } = setup();
    void change('one');
    await answer();
    const undoing = manager.undo();
    await flush(); // the undo of "one" is being written
    void change('two'); // under way too
    manager.clear();
    await answer();
    await answer();
    await undoing;
    expect(manager.canUndo).toBe(false);
    expect(manager.canRedo).toBe(false);
  });

  it('does not let a failed undo under way put its step into the new history', async () => {
    const { manager, change, answer, refuse, flush } = setup();
    void change('one');
    await answer();
    const undoing = manager.undo().catch((error: unknown) => error);
    await flush();
    manager.clear();
    void change('new');
    await refuse(); // the undo of "one"
    expect(await undoing).toEqual(new Error('refused'));
    await answer(); // "new"
    expect(manager.canUndo).toBe(true);
    await Promise.all([manager.undo(), answer()]);
    expect(manager.canUndo).toBe(false);
    expect(manager.canRedo).toBe(true);
  });

  it('does not remember a gesture begun before it', async () => {
    const manager = new UndoManager();
    const gate = held();
    const gesture = manager.group(async (groupId) => {
      await gate.promise;
      await manager.perform(
        async () => undefined,
        async () => undefined,
        'late',
        groupId,
      );
    });
    manager.clear();
    gate.answer();
    await gesture;
    expect(manager.canUndo).toBe(false);
  });

  it('does not keep the new history waiting for an undo of the one left', async () => {
    const { manager, change, answer, answerAt, flush, writes } = setup();
    void change('one');
    await answer();
    void manager.undo(); // its write never answers
    await flush();
    manager.clear();
    expect(manager.busy).toBe(false);
    void change('new');
    await answerAt(1); // "new"
    const undoing = manager.undo();
    await flush();
    // The undo of "new" is sent at once: it does not wait for the one of the history left.
    expect(writes).toHaveLength(2);
    await answerAt(1);
    await undoing;
    expect(manager.busy).toBe(false);
    expect(manager.canRedo).toBe(true);
  });
});

describe('changes refused one after the other (found by a review of 042ff43)', () => {
  it.each([
    { first: 0, second: 1 },
    { first: 1, second: 0 },
  ])(
    'gives back what could be redone when two changes made after an undo are refused, $first then $second',
    async ({ first, second }) => {
      const manager = new UndoManager();
      let value = 'start';
      await manager.perform(
        async () => {
          value = 'original';
        },
        async () => {
          value = 'start';
        },
        'original',
      );
      await manager.undo();
      const writes = [held(), held()];
      const changes = writes.map((write) =>
        manager
          .perform(
            () => write.promise,
            async () => undefined,
          )
          .catch(() => undefined),
      );
      writes[first]?.refuse();
      await changes[first];
      writes[second]?.refuse();
      await changes[second];

      expect(manager.canUndo).toBe(false);
      expect(manager.redoDescription).toBe('original');
      await manager.redo();
      expect(value).toBe('original');
    },
  );

  it('does not give back an empty step to redo when an undo and the newer change are both refused', async () => {
    const manager = new UndoManager();
    const undoWrite = held();
    const changeWrite = held();
    let entered = (): void => undefined;
    const started = new Promise<void>((resolve) => {
      entered = resolve;
    });
    await manager.perform(
      async () => undefined,
      async () => {
        entered();
        await undoWrite.promise;
      },
      'original',
    );

    const undoing = manager.undo().catch(() => undefined);
    await started;
    const changing = manager
      .perform(
        () => changeWrite.promise,
        async () => undefined,
        'newer',
      )
      .catch(() => undefined);
    undoWrite.refuse();
    await undoing;
    changeWrite.refuse();
    await changing;

    expect(manager.undoDescription).toBe('original');
    expect(manager.canRedo).toBe(false);
  });

  it('keeps two gestures placed at once in the order asked, and an undo asked meanwhile for the last', async () => {
    // Two duplicates, each reading its card before it makes the copy.
    const manager = new UndoManager();
    const values: string[] = [];
    const reads = [held(), held()];
    const copying = ['x', 'y'].map((value, at) =>
      manager.group(async (groupId) => {
        await reads[at]?.promise;
        await adding(manager, values, value, groupId);
      }, true),
    );
    const undone = manager.undo();
    reads[0]?.answer();
    await copying[0];
    reads[1]?.answer();
    await Promise.all([...copying, undone]);
    expect(values).toEqual(['x']);
    expect(manager.undoDescription).toBe('x');
    expect(manager.redoDescription).toBe('y');
  });

  it('leaves no step of a gesture placed at once that changed nothing, and gives back what could be redone', async () => {
    const manager = new UndoManager();
    const values: string[] = [];
    await adding(manager, values, 'a');
    await manager.undo();
    const read = held();
    const copying = manager.group(async (groupId) => {
      await read.promise;
      await adding(manager, values, 'never', groupId);
    }, true);
    expect(manager.canRedo).toBe(false);
    read.refuse();
    await copying.catch(() => undefined);
    expect(manager.canUndo).toBe(false);
    expect(manager.redoDescription).toBe('a');
  });
});
