/**
 * Undo and redo: single steps, steps that depend on what an action answered,
 * and groups undone as one. Every case has a manager of its own.
 */
import { expect, test } from 'vitest';

import { UndoManager } from '$lib/logic/undo.ts';

test('Basic undo/redo', async () => {
  const undoManager = new UndoManager();
  expect(undoManager.canUndo).toBe(false);
  expect(undoManager.canRedo).toBe(false);
  expect(undoManager.currentCheckpoint).toBe(0);

  const values: string[] = [];

  await undoManager.perform(
    async () => {
      values.push('a');
    },
    async () => {
      values.pop();
    },
    'test',
  );

  expect(undoManager.canUndo).toBe(true);
  expect(undoManager.canRedo).toBe(false);
  expect(undoManager.currentCheckpoint).toBeGreaterThan(0);
  expect(values).toEqual(['a']);
  expect(undoManager.undoDescription).toBe('test');
  expect(undoManager.redoDescription).toBe(undefined);

  await undoManager.undo();
  expect(undoManager.canUndo).toBe(false);
  expect(undoManager.canRedo).toBe(true);
  expect(values).toEqual([]);
  expect(undoManager.undoDescription).toBe(undefined);
  expect(undoManager.redoDescription).toBe('test');

  await undoManager.redo();
  expect(undoManager.canUndo).toBe(true);
  expect(undoManager.canRedo).toBe(false);
  expect(values).toEqual(['a']);

  undoManager.clear();
  expect(undoManager.canUndo).toBe(false);
  expect(undoManager.canRedo).toBe(false);
  expect(undoManager.currentCheckpoint).toBe(0);
  expect(undoManager.undoDescription).toBe(undefined);
  expect(undoManager.redoDescription).toBe(undefined);
});

test('Basic undo/redo response dependant', async () => {
  const undoManager = new UndoManager();
  expect(undoManager.canUndo).toBe(false);
  expect(undoManager.canRedo).toBe(false);
  expect(undoManager.currentCheckpoint).toBe(0);

  interface Sample {
    id: number;
    title: string;
  }
  const blockIds = [2, 1];
  const blocks: Partial<Record<number, Sample>> = {};

  const newBlock = await undoManager.perform(
    async () => {
      // every time we run the action a new ID is obtained
      const block: Sample = { id: blockIds.pop() ?? 0, title: 'Sample' };
      blocks[block.id] = block;
      return block;
    },
    async (block: Sample) => {
      blocks[block.id] = undefined;
    },
    'test',
  );

  // should insert the block and return the new block for its use
  expect(undoManager.canUndo).toBe(true);
  expect(undoManager.canRedo).toBe(false);
  expect(blocks[1]).toBeDefined();
  expect(blocks[1]).toEqual(newBlock);

  // should correctly remove the block based on the info gathered in
  // the redo function
  await undoManager.undo();
  expect(undoManager.canUndo).toBe(false);
  expect(undoManager.canRedo).toBe(true);
  expect(blocks[1]).toBeUndefined();

  // when redoing, as the function has side effects the new id will
  // be different
  await undoManager.redo();
  expect(undoManager.canUndo).toBe(true);
  expect(undoManager.canRedo).toBe(false);
  expect(blocks[1]).toBeUndefined();
  expect(blocks[2]).toBeDefined();
  expect(blocks[2]?.id).toEqual(2);

  // when undoing, the undo manager has saved the new id internally
  // and it removes the right block
  await undoManager.undo();
  expect(undoManager.canUndo).toBe(false);
  expect(undoManager.canRedo).toBe(true);
  expect(blocks[2]).toBeUndefined();

  undoManager.clear();
});

test('Grouped undo/redo', async () => {
  const undoManager = new UndoManager();
  expect(undoManager.canUndo).toBe(false);
  expect(undoManager.canRedo).toBe(false);

  const values: string[] = [];
  const groupId = 'the group id';

  await undoManager.perform(
    async () => {
      values.push('a');
    },
    async () => {
      values.pop();
    },
    'insert a',
  );

  expect(undoManager.canUndo).toBe(true);
  expect(undoManager.canRedo).toBe(false);
  expect(values).toEqual(['a']);
  expect(undoManager.undoDescription).toBe('insert a');
  expect(undoManager.redoDescription).toBe(undefined);

  await undoManager.perform(
    async () => {
      values.push('b');
    },
    async () => {
      values.pop();
    },
    'insert b',
    groupId,
  );

  expect(undoManager.canUndo).toBe(true);
  expect(undoManager.canRedo).toBe(false);
  expect(values).toEqual(['a', 'b']);
  expect(undoManager.undoDescription).toBe('insert b');
  expect(undoManager.redoDescription).toBe(undefined);

  await undoManager.perform(
    async () => {
      values.push('c');
    },
    async () => {
      values.pop();
    },
    'insert c',
    groupId,
  );

  expect(undoManager.canUndo).toBe(true);
  expect(undoManager.canRedo).toBe(false);
  expect(values).toEqual(['a', 'b', 'c']);
  expect(undoManager.undoDescription).toBe('insert c');
  expect(undoManager.redoDescription).toBe(undefined);

  await undoManager.undo();
  expect(undoManager.canUndo).toBe(true);
  expect(undoManager.canRedo).toBe(true);
  expect(values).toEqual(['a']);
  expect(undoManager.undoDescription).toBe('insert a');
  expect(undoManager.redoDescription).toBe('insert b');

  await undoManager.redo();
  expect(undoManager.canUndo).toBe(true);
  expect(undoManager.canRedo).toBe(false);
  expect(values).toEqual(['a', 'b', 'c']);
  expect(undoManager.undoDescription).toBe('insert c');
  expect(undoManager.redoDescription).toBe(undefined);

  undoManager.clear();
  expect(undoManager.canUndo).toBe(false);
  expect(undoManager.canRedo).toBe(false);
  expect(undoManager.undoDescription).toBe(undefined);
  expect(undoManager.redoDescription).toBe(undefined);
});
