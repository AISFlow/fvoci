/**
 * The patches made from two blocks or two boards, filter clauses compared,
 * and views sorted by name. What is expected is written out in full, not kept
 * as stored snapshots.
 *
 * This file tests wire.ts and is, with the fixtures, the only test file that
 * may reach into the `fields` of a block.
 */
import { describe, expect, it, test } from 'vitest';

import { areEqual } from '$lib/logic/filter.ts';
import { sortBoardViewsAlphabetically } from '$lib/logic/sort.ts';
import {
  createBlock,
  createBoard,
  createFilterClause,
  createPatchesFromBlocks,
  createPatchesFromContainers,
  type PropertyTemplate,
} from '$lib/logic/wire.ts';

import {
  createTestBoard,
  createTestBoardView,
  createTestCard,
  createTestText,
} from './support/factory.ts';

describe('block tests', () => {
  const board = createTestBoard();
  const card = createTestCard(board);

  describe('correctly generate patches from two blocks', () => {
    it('should generate two empty patches for the same block', () => {
      const textBlock = createTestText(card);
      const result = createPatchesFromBlocks(textBlock, textBlock);
      expect(result).toEqual([
        {
          deletedFields: [],
          updatedFields: {},
        },
        {
          deletedFields: [],
          updatedFields: {},
        },
      ]);
    });

    it('should add fields on the new fields added and remove it in the undo', () => {
      const oldBlock = createTestText(card);
      const newBlock = createBlock(oldBlock);
      newBlock.fields.newField = 'new field';
      const result = createPatchesFromBlocks(newBlock, oldBlock);
      expect(result).toEqual([
        {
          deletedFields: [],
          updatedFields: {
            newField: 'new field',
          },
        },
        {
          deletedFields: ['newField'],
          updatedFields: {},
        },
      ]);
    });

    it('should remove field on the new block added and add it again in the undo', () => {
      const oldBlock = createTestText(card);
      const newBlock = createBlock(oldBlock);
      oldBlock.fields.test = 'test';
      const result = createPatchesFromBlocks(newBlock, oldBlock);
      expect(result).toEqual([
        {
          deletedFields: ['test'],
          updatedFields: {},
        },
        {
          deletedFields: [],
          updatedFields: {
            test: 'test',
          },
        },
      ]);
    });

    it('should update propertie on the main object and revert it back on the undo', () => {
      const oldBlock = createTestText(card);
      const newBlock = createBlock(oldBlock);
      oldBlock.parentId = 'old-parent-id';
      newBlock.parentId = 'new-parent-id';
      const result = createPatchesFromBlocks(newBlock, oldBlock);
      expect(result).toEqual([
        {
          deletedFields: [],
          parentId: 'new-parent-id',
          updatedFields: {},
        },
        {
          deletedFields: [],
          parentId: 'old-parent-id',
          updatedFields: {},
        },
      ]);
    });
  });
});

describe('board tests', () => {
  describe('correctly generate patches from two boards', () => {
    it('should generate two empty patches for the same board', () => {
      const board = createTestBoard();
      const result = createPatchesFromContainers(board, board);
      expect(result).toEqual([
        {
          deletedCardProperties: [],
          deletedProperties: [],
          updatedCardProperties: [],
          updatedProperties: {},
        },
        {
          deletedCardProperties: [],
          deletedProperties: [],
          updatedCardProperties: [],
          updatedProperties: {},
        },
      ]);
    });

    it('should add properties on the update patch and remove them on the undo', () => {
      const board = createTestBoard();
      board.properties = {
        prop1: 'val1',
        prop2: 'val2',
      };
      const oldBoard = createBoard(board);
      oldBoard.properties = {
        prop2: 'val2',
      };

      const result = createPatchesFromContainers(board, oldBoard);
      expect(result).toEqual([
        {
          deletedCardProperties: [],
          deletedProperties: [],
          updatedCardProperties: [],
          updatedProperties: {
            prop1: 'val1',
          },
        },
        {
          deletedCardProperties: [],
          deletedProperties: ['prop1'],
          updatedCardProperties: [],
          updatedProperties: {},
        },
      ]);
    });

    it('should add card properties on the redo and remove them on the undo', () => {
      const board = createTestBoard();
      const oldBoard = createBoard(board);
      board.cardProperties.push({
        id: 'new-property-id',
        name: 'property-name',
        type: 'select',
        options: [
          {
            id: 'opt',
            label: 'val',
            color: 'yellow',
          },
        ],
      });

      const result = createPatchesFromContainers(board, oldBoard);
      expect(result).toEqual([
        {
          deletedCardProperties: [],
          deletedProperties: [],
          updatedCardProperties: [
            {
              id: 'new-property-id',
              name: 'property-name',
              options: [
                {
                  color: 'yellow',
                  id: 'opt',
                  label: 'val',
                },
              ],
              type: 'select',
            },
          ],
          updatedProperties: {},
        },
        {
          deletedCardProperties: ['new-property-id'],
          deletedProperties: [],
          updatedCardProperties: [],
          updatedProperties: {},
        },
      ]);
    });

    it('should add card properties on the redo and undo if they exists in both, but differ', () => {
      const cardProperty = {
        id: 'new-property-id',
        name: 'property-name',
        type: 'select',
        options: [
          {
            id: 'opt',
            label: 'val',
            color: 'yellow',
          },
        ],
      } as PropertyTemplate;

      const board = createTestBoard();
      const oldBoard = createBoard(board);
      board.cardProperties = [cardProperty];
      oldBoard.cardProperties = [{ ...cardProperty, name: 'a-different-name' }];

      const result = createPatchesFromContainers(board, oldBoard);
      expect(result).toEqual([
        {
          deletedCardProperties: [],
          deletedProperties: [],
          updatedCardProperties: [
            {
              id: 'new-property-id',
              name: 'property-name',
              options: [
                {
                  color: 'yellow',
                  id: 'opt',
                  label: 'val',
                },
              ],
              type: 'select',
            },
          ],
          updatedProperties: {},
        },
        {
          deletedCardProperties: [],
          deletedProperties: [],
          updatedCardProperties: [
            {
              id: 'new-property-id',
              name: 'a-different-name',
              options: [
                {
                  color: 'yellow',
                  id: 'opt',
                  label: 'val',
                },
              ],
              type: 'select',
            },
          ],
          updatedProperties: {},
        },
      ]);
    });

    it('should add card properties on the redo and undo if they exists in both, but their options are different', () => {
      const cardProperty = {
        id: 'new-property-id',
        name: 'property-name',
        type: 'select',
        options: [
          {
            id: 'opt',
            label: 'val',
            color: 'yellow',
          },
        ],
      } as PropertyTemplate;

      const board = createTestBoard();
      const oldBoard = createBoard(board);
      board.cardProperties = [cardProperty];
      oldBoard.cardProperties = [
        {
          ...cardProperty,
          options: [
            {
              id: 'another-opt',
              label: 'val',
              color: 'brown',
            },
          ],
        },
      ];

      const result = createPatchesFromContainers(board, oldBoard);
      expect(result).toEqual([
        {
          deletedCardProperties: [],
          deletedProperties: [],
          updatedCardProperties: [
            {
              id: 'new-property-id',
              name: 'property-name',
              options: [
                {
                  color: 'yellow',
                  id: 'opt',
                  label: 'val',
                },
              ],
              type: 'select',
            },
          ],
          updatedProperties: {},
        },
        {
          deletedCardProperties: [],
          deletedProperties: [],
          updatedCardProperties: [
            {
              id: 'new-property-id',
              name: 'property-name',
              options: [
                {
                  color: 'brown',
                  id: 'another-opt',
                  label: 'val',
                },
              ],
              type: 'select',
            },
          ],
          updatedProperties: {},
        },
      ]);
    });
  });
});

describe('filterClause tests', () => {
  it('create filter clause', () => {
    const clause = createFilterClause({
      propertyId: 'myPropertyId',
      condition: 'contains',
      values: [],
    });

    expect(clause).toEqual({
      propertyId: 'myPropertyId',
      condition: 'contains',
      values: [],
    });
  });

  it('test filter clauses are equal', () => {
    const clause = createFilterClause({
      propertyId: 'myPropertyId',
      condition: 'contains',
      values: ['abc', 'def'],
    });
    const newClause = createFilterClause(clause);
    const testEqual = areEqual(clause, newClause);
    expect(testEqual).toBeTruthy();
  });

  it('test filter clauses are Not equal property ID', () => {
    const clause = createFilterClause({
      propertyId: 'myPropertyId',
      condition: 'contains',
      values: ['abc', 'def'],
    });
    const newClause = createFilterClause(clause);
    newClause.propertyId = 'DifferentID';
    const testEqual = areEqual(clause, newClause);
    expect(testEqual).toBeFalsy();
  });
  it('test filter clauses are Not equal condition', () => {
    const clause = createFilterClause({
      propertyId: 'myPropertyId',
      condition: 'contains',
      values: ['abc', 'def'],
    });
    const newClause = createFilterClause(clause);
    newClause.condition = 'notContains';
    const testEqual = areEqual(clause, newClause);
    expect(testEqual).toBeFalsy();
  });
  it('test filter clauses are Not equal values', () => {
    const clause = createFilterClause({
      propertyId: 'myPropertyId',
      condition: 'contains',
      values: ['abc', 'def'],
    });
    const newClause = createFilterClause(clause);
    newClause.values = ['abc, def'];
    const testEqual = areEqual(clause, newClause);
    expect(testEqual).toBeFalsy();
  });
});

test('boardView: sort with ASCII', () => {
  const view1 = createTestBoardView();
  view1.title = 'Maybe';
  const view2 = createTestBoardView();
  view2.title = 'Active';

  const views = [view1, view2];
  const sorted = sortBoardViewsAlphabetically(views);
  expect(sorted).toEqual([view2, view1]);
});

test('boardView: sort with leading emoji', () => {
  const view1 = createTestBoardView();
  view1.title = '🤔 Maybe';
  const view2 = createTestBoardView();
  view2.title = '🚀 Active';

  const views = [view1, view2];
  const sorted = sortBoardViewsAlphabetically(views);
  expect(sorted).toEqual([view2, view1]);
});

test('boardView: sort with non-latin characters', () => {
  const view1 = createTestBoardView();
  view1.title = 'zebra';
  const view2 = createTestBoardView();
  view2.title = 'ñu';

  const views = [view1, view2];
  const sorted = sortBoardViewsAlphabetically(views);
  expect(sorted).toEqual([view2, view1]);
});
