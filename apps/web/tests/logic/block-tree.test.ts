/**
 * Copying a tree of blocks (a copy takes new identifiers, keeps its order and
 * drops what marks a block deleted), and the default of a filter condition.
 */
import { expect, test } from 'vitest';

import { duplicateBlockTree } from '$lib/logic/block-tree.ts';
import { filterConditionValidOrDefault } from '$lib/logic/filter-conditions.ts';
import type { Block } from '$lib/logic/wire.ts';

import { createTestCard, createTestText } from './support/factory.ts';

test('duplicateBlockTree: Card', () => {
  const [blocks, sourceBlock] = createCardTree();

  const [newBlocks, newSourceBlock, idMap] = duplicateBlockTree(blocks, sourceBlock.id);

  expect(newBlocks.length).toBe(blocks.length);
  expect(newSourceBlock.id).not.toBe(sourceBlock.id);
  expect(newSourceBlock.type).toBe(sourceBlock.type);

  // When duplicating a non-root block, the boardId should not be re-mapped
  expect(newSourceBlock.containerId).toBe(sourceBlock.containerId);
  expect(idMap[sourceBlock.id]).toBe(newSourceBlock.id);

  for (const newBlock of newBlocks) {
    expect(newBlock.containerId).toBe(newSourceBlock.containerId);
  }

  for (const textBlock of newBlocks.filter((o) => o.type === 'text')) {
    expect(textBlock.parentId).toBe(newSourceBlock.id);
  }
});

test('duplicateBlockTree strips deletedAt from copied blocks', () => {
  const [blocks, sourceBlock] = createCardTree();
  for (const block of blocks) {
    block.deletedAt = 12345;
  }
  const [newBlocks, newSourceBlock] = duplicateBlockTree(blocks, sourceBlock.id);
  expect(newSourceBlock.deletedAt).toBeUndefined();
  for (const copied of newBlocks) {
    expect(copied.deletedAt).toBeUndefined();
  }
});

test('filterConditionValidOrDefault', () => {
  // Test 'options'
  expect(filterConditionValidOrDefault('options', 'includes')).toBe('includes');
  expect(filterConditionValidOrDefault('options', 'notIncludes')).toBe('notIncludes');
  expect(filterConditionValidOrDefault('options', 'isEmpty')).toBe('isEmpty');
  expect(filterConditionValidOrDefault('options', 'isNotEmpty')).toBe('isNotEmpty');
  expect(filterConditionValidOrDefault('options', 'is')).toBe('includes');

  expect(filterConditionValidOrDefault('boolean', 'isSet')).toBe('isSet');
  expect(filterConditionValidOrDefault('boolean', 'isNotSet')).toBe('isNotSet');
  expect(filterConditionValidOrDefault('boolean', 'includes')).toBe('isSet');

  expect(filterConditionValidOrDefault('text', 'is')).toBe('is');
  expect(filterConditionValidOrDefault('text', 'contains')).toBe('contains');
  expect(filterConditionValidOrDefault('text', 'notContains')).toBe('notContains');
  expect(filterConditionValidOrDefault('text', 'startsWith')).toBe('startsWith');
  expect(filterConditionValidOrDefault('text', 'notStartsWith')).toBe('notStartsWith');
  expect(filterConditionValidOrDefault('text', 'endsWith')).toBe('endsWith');
  expect(filterConditionValidOrDefault('text', 'notEndsWith')).toBe('notEndsWith');
  expect(filterConditionValidOrDefault('text', 'isEmpty')).toBe('is');

  expect(filterConditionValidOrDefault('date', 'is')).toBe('is');
  expect(filterConditionValidOrDefault('date', 'isBefore')).toBe('isBefore');
  expect(filterConditionValidOrDefault('date', 'isAfter')).toBe('isAfter');
  expect(filterConditionValidOrDefault('date', 'isSet')).toBe('isSet');
  expect(filterConditionValidOrDefault('date', 'isNotSet')).toBe('isNotSet');
  expect(filterConditionValidOrDefault('date', 'isEmpty')).toBe('is');
});

function createCardTree(): [Block[], Block] {
  const blocks: Block[] = [];

  const card = createTestCard();
  card.id = 'card1';
  card.containerId = 'board1';
  blocks.push(card);

  for (let i = 0; i < 5; i++) {
    const textBlock = createTestText(card);
    textBlock.id = `text${i}`;
    blocks.push(textBlock);
  }

  return [blocks, card];
}
