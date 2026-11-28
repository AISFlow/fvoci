/**
 * The filter of cards: a clause on a property of options, text, people or
 * dates, groups of clauses, and the properties a new card takes from a filter.
 */
import { beforeEach, describe, expect, test, vi } from 'vitest';

import { addDays, calendarDate, dateOfUtc } from '$lib/logic/dates.ts';
import { diagnostics } from '$lib/logic/diagnostics.ts';
import {
  applyFilterGroup,
  isClauseMet,
  isFilterGroupMet,
  propertiesThatMeetFilterGroup,
  propertyThatMeetsFilterClause,
} from '$lib/logic/filter.ts';
import {
  createFilterClause,
  createFilterGroup,
  setCardProperty,
  type PropertyTemplate,
} from '$lib/logic/wire.ts';

import { at } from '../support/zones.ts';

import { createTestBoard, createTestCard } from './support/factory.ts';

// The person reading the board, in a time zone of their own wherever the tests run.
const reader = { timeZone: 'Asia/Seoul' };

describe('the filter of cards', () => {
  const report = vi.spyOn(diagnostics, 'report').mockImplementation(() => undefined);
  beforeEach(() => {
    report.mockClear();
  });

  const board = createTestBoard();
  board.id = '1';

  const card1 = createTestCard(board);
  card1.id = '1';
  card1.title = 'card1';
  setCardProperty(card1, 'propertyId', 'Status');

  card1.createdAt = at(reader.timeZone, 2023, 12, 7);
  card1.updatedAt = at(reader.timeZone, 2023, 12, 7);
  card1.deletedAt = at(reader.timeZone, 2023, 12, 7);
  const filterClause = createFilterClause({
    propertyId: 'propertyId',
    condition: 'isNotEmpty',
    values: ['Status'],
  });

  describe('verify isClauseMet method', () => {
    test('should be true with isNotEmpty clause', () => {
      const filterClauseIsNotEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'isNotEmpty',
        values: ['Status'],
      });
      const result = isClauseMet(filterClauseIsNotEmpty, [], card1, reader);
      expect(result).toBeTruthy();
    });
    test('should be false with isEmpty clause', () => {
      const filterClauseIsEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'isEmpty',
        values: ['Status'],
      });
      const result = isClauseMet(filterClauseIsEmpty, [], card1, reader);
      expect(result).toBeFalsy();
    });
    test('should be true with includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'includes',
        values: ['Status'],
      });
      const result = isClauseMet(filterClauseIncludes, [], card1, reader);
      expect(result).toBeTruthy();
    });
    test('should be true with includes and no values clauses', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'includes',
        values: [],
      });
      const result = isClauseMet(filterClauseIncludes, [], card1, reader);
      expect(result).toBeTruthy();
    });
    test('should be false with notIncludes clause', () => {
      const filterClauseNotIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'notIncludes',
        values: ['Status'],
      });
      const result = isClauseMet(filterClauseNotIncludes, [], card1, reader);
      expect(result).toBeFalsy();
    });
    test('should be true with notIncludes and no values clauses', () => {
      const filterClauseNotIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'notIncludes',
        values: [],
      });
      const result = isClauseMet(filterClauseNotIncludes, [], card1, reader);
      expect(result).toBeTruthy();
    });
  });

  describe('verify isClauseMet method - person property', () => {
    const personCard = createTestCard(board);
    personCard.id = '1';
    personCard.title = 'card1';
    setCardProperty(personCard, 'personPropertyID', 'personid1');

    const template: PropertyTemplate = {
      id: 'personPropertyID',
      name: 'myPerson',
      type: 'person',
      options: [],
    };

    test('should be true with isNotEmpty clause', () => {
      const filterClauseIsNotEmpty = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'isNotEmpty',
        values: [],
      });
      const result = isClauseMet(filterClauseIsNotEmpty, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('should be false with isEmpty clause', () => {
      const filterClauseIsEmpty = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'isEmpty',
        values: [],
      });
      const result = isClauseMet(filterClauseIsEmpty, [template], personCard, reader);
      expect(result).toBeFalsy();
    });
    test('verify empty includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: [],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: ['personid1'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify includes clause multiple values', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: ['personid2', 'personid1'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify not includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'notIncludes',
        values: ['personid2'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
  });

  describe('verify isClauseMet method - multi-person property', () => {
    const personCard = createTestCard(board);
    personCard.id = '1';
    personCard.title = 'card1';
    setCardProperty(personCard, 'personPropertyID', ['personid1', 'personid2']);

    const template: PropertyTemplate = {
      id: 'personPropertyID',
      name: 'myPerson',
      type: 'multiPerson',
      options: [],
    };

    test('should be true with isNotEmpty clause', () => {
      const filterClauseIsNotEmpty = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'isNotEmpty',
        values: [],
      });
      const result = isClauseMet(filterClauseIsNotEmpty, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('should be false with isEmpty clause', () => {
      const filterClauseIsEmpty = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'isEmpty',
        values: [],
      });
      const result = isClauseMet(filterClauseIsEmpty, [template], personCard, reader);
      expect(result).toBeFalsy();
    });
    test('verify empty includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: [],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: ['personid1'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify includes clause 2', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: ['personid2'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify includes clause multiple values', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: ['personid3', 'personid1'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify includes clause multiple values 2', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: ['personid3', 'personid2'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify not includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'notIncludes',
        values: ['personid3'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify not includes clause, multiple values', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'notIncludes',
        values: ['personid3', 'personid4'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
  });

  describe('verify isClauseMet method - (createdBy) person property', () => {
    const personCard = createTestCard(board);
    personCard.id = '1';
    personCard.title = 'card1';
    personCard.createdBy = 'personid1';

    const template: PropertyTemplate = {
      id: 'personPropertyID',
      name: 'myPerson',
      type: 'createdBy',
      options: [],
    };

    test('verify empty includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: [],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: ['personid1'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify includes clause multiple values', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'includes',
        values: ['personid3', 'personid1'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
    test('verify not includes clause', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'personPropertyID',
        condition: 'notIncludes',
        values: ['personid2'],
      });
      const result = isClauseMet(filterClauseIncludes, [template], personCard, reader);
      expect(result).toBeTruthy();
    });
  });

  describe('verify isClauseMet method - single date property', () => {
    // A date property holds a calendar day, and so does a date filter.
    const propertyDate = dateOfUtc(Date.now());

    const dateCard = createTestCard(board);
    dateCard.id = '1';
    dateCard.title = 'card1';
    setCardProperty(dateCard, 'datePropertyID', { from: propertyDate });

    const checkDayBefore = addDays(propertyDate, -1);
    const checkDayAfter = addDays(propertyDate, 1);

    const template: PropertyTemplate = {
      id: 'datePropertyID',
      name: 'myDate',
      type: 'date',
      options: [],
    };

    test('should be true with isSet clause', () => {
      const filterClauseIsSet = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isSet',
        values: [],
      });
      const result = isClauseMet(filterClauseIsSet, [template], dateCard, reader);
      expect(result).toBeTruthy();
    });
    test('should be false with notSet clause', () => {
      const filterClauseIsNotSet = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isNotSet',
        values: [],
      });
      const result = isClauseMet(filterClauseIsNotSet, [template], dateCard, reader);
      expect(result).toBeFalsy();
    });
    test('verify isBefore clause', () => {
      const filterClauseIsBefore = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [checkDayAfter.toString()],
      });
      const result = isClauseMet(filterClauseIsBefore, [template], dateCard, reader);
      expect(result).toBeTruthy();

      const filterClauseIsNotBefore = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [checkDayBefore.toString()],
      });
      const result2 = isClauseMet(filterClauseIsNotBefore, [template], dateCard, reader);
      expect(result2).toBeFalsy();
    });
    test('verify isAfter clauses', () => {
      const filterClauseisAfter = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isAfter',
        values: [checkDayBefore.toString()],
      });
      const result = isClauseMet(filterClauseisAfter, [template], dateCard, reader);
      expect(result).toBeTruthy();

      const filterClauseisNotAfter = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isAfter',
        values: [checkDayAfter.toString()],
      });
      const result2 = isClauseMet(filterClauseisNotAfter, [template], dateCard, reader);
      expect(result2).toBeFalsy();
    });
    test('verify is clause', () => {
      const filterClauseIs = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'is',
        values: [propertyDate.toString()],
      });
      const result = isClauseMet(filterClauseIs, [template], dateCard, reader);
      expect(result).toBeTruthy();

      const filterClauseIsNot = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'is',
        values: [checkDayBefore.toString()],
      });
      const result2 = isClauseMet(filterClauseIsNot, [template], dateCard, reader);
      expect(result2).toBeFalsy();
    });
  });

  describe('verify isClauseMet method - date range property', () => {
    // A date property holds calendar days, and so does a date filter.
    const fromDate = dateOfUtc(Date.now());
    const toDate = addDays(fromDate, 2);
    const dateCard = createTestCard(board);
    dateCard.id = '1';
    dateCard.title = 'card1';
    setCardProperty(dateCard, 'datePropertyID', { from: fromDate, to: toDate });

    const beforeRange = addDays(fromDate, -1);
    const afterRange = addDays(toDate, 1);
    const inRange = addDays(fromDate, 1);

    const template: PropertyTemplate = {
      id: 'datePropertyID',
      name: 'myDate',
      type: 'date',
      options: [],
    };

    test('verify isBefore clause', () => {
      const filterClauseIsBeforeEmpty = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [],
      });
      const resulta = isClauseMet(filterClauseIsBeforeEmpty, [template], dateCard, reader);
      expect(resulta).toBeTruthy();

      const filterClauseIsBefore = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [beforeRange.toString()],
      });
      const result = isClauseMet(filterClauseIsBefore, [template], dateCard, reader);
      expect(result).toBeFalsy();

      const filterClauseIsInRange = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [inRange.toString()],
      });
      const result2 = isClauseMet(filterClauseIsInRange, [template], dateCard, reader);
      expect(result2).toBeTruthy();

      const filterClauseIsAfter = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [afterRange.toString()],
      });
      const result3 = isClauseMet(filterClauseIsAfter, [template], dateCard, reader);
      expect(result3).toBeTruthy();
    });

    test('verify isAfter clauses', () => {
      const filterClauseIsAfterEmpty = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [],
      });
      const resulta = isClauseMet(filterClauseIsAfterEmpty, [template], dateCard, reader);
      expect(resulta).toBeTruthy();

      const filterClauseIsAfter = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isAfter',
        values: [afterRange.toString()],
      });
      const result = isClauseMet(filterClauseIsAfter, [template], dateCard, reader);
      expect(result).toBeFalsy();

      const filterClauseIsInRange = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isAfter',
        values: [inRange.toString()],
      });
      const result2 = isClauseMet(filterClauseIsInRange, [template], dateCard, reader);
      expect(result2).toBeTruthy();

      const filterClauseIsBefore = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isAfter',
        values: [beforeRange.toString()],
      });
      const result3 = isClauseMet(filterClauseIsBefore, [template], dateCard, reader);
      expect(result3).toBeTruthy();
    });

    test('verify is clause', () => {
      const filterClauseIsEmpty = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [],
      });
      const resulta = isClauseMet(filterClauseIsEmpty, [template], dateCard, reader);
      expect(resulta).toBeTruthy();

      const filterClauseIsBefore = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'is',
        values: [beforeRange.toString()],
      });
      const result = isClauseMet(filterClauseIsBefore, [template], dateCard, reader);
      expect(result).toBeFalsy();

      const filterClauseIsInRange = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'is',
        values: [inRange.toString()],
      });
      const result2 = isClauseMet(filterClauseIsInRange, [template], dateCard, reader);
      expect(result2).toBeTruthy();

      const filterClauseIsAfter = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'is',
        values: [afterRange.toString()],
      });
      const result3 = isClauseMet(filterClauseIsAfter, [template], dateCard, reader);
      expect(result3).toBeFalsy();
    });
  });

  // Q16 in docs/notes.md: the filter compares calendar days in the reader's
  // time zone, so these three cases hold in every time zone.
  describe('verify isClauseMet method - (createdTime) date property', () => {
    // The day the card was created on, for the reader: 7 December 2023.
    const checkDate = calendarDate(2023, 12, 7);
    const checkDayBefore = addDays(checkDate, -1);
    const checkDayAfter = addDays(checkDate, 1);

    const template: PropertyTemplate = {
      id: 'datePropertyID',
      name: 'myDate',
      type: 'createdTime',
      options: [],
    };

    test('should be true with isSet clause', () => {
      const filterClauseIsSet = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isSet',
        values: [],
      });
      const result = isClauseMet(filterClauseIsSet, [template], card1, reader);
      expect(result).toBeTruthy();
    });
    test('should be false with notSet clause', () => {
      const filterClauseIsNotSet = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isNotSet',
        values: [],
      });
      const result = isClauseMet(filterClauseIsNotSet, [template], card1, reader);
      expect(result).toBeFalsy();
    });
    test('verify isBefore clause', () => {
      const filterClauseIsBefore = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [checkDayAfter.toString()],
      });
      const result = isClauseMet(filterClauseIsBefore, [template], card1, reader);
      expect(result).toBeTruthy();

      const filterClauseIsNotBefore = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isBefore',
        values: [checkDate.toString()],
      });
      const result2 = isClauseMet(filterClauseIsNotBefore, [template], card1, reader);
      expect(result2).toBeFalsy();
    });
    test('verify isAfter clauses', () => {
      const filterClauseisAfter = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isAfter',
        values: [checkDayBefore.toString()],
      });
      const result = isClauseMet(filterClauseisAfter, [template], card1, reader);
      expect(result).toBeTruthy();

      const filterClauseisNotAfter = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'isAfter',
        values: [checkDate.toString()],
      });
      const result2 = isClauseMet(filterClauseisNotAfter, [template], card1, reader);
      expect(result2).toBeFalsy();
    });
    test('verify is clause', () => {
      // Is should find on that date regardless of time.
      const filterClauseIs = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'is',
        values: [checkDate.toString()],
      });
      const result = isClauseMet(filterClauseIs, [template], card1, reader);
      expect(result).toBeTruthy();

      const filterClauseIsNot = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'is',
        values: [checkDayBefore.toString()],
      });
      const result2 = isClauseMet(filterClauseIsNot, [template], card1, reader);
      expect(result2).toBeFalsy();

      const filterClauseIsNot2 = createFilterClause({
        propertyId: 'datePropertyID',
        condition: 'is',
        values: [checkDayAfter.toString()],
      });
      const result3 = isClauseMet(filterClauseIsNot2, [template], card1, reader);
      expect(result3).toBeFalsy();
    });
  });

  describe('verify isFilterGroupMet method', () => {
    test('should return true with no filter', () => {
      const filterGroup = createFilterGroup({
        operation: 'and',
        filters: [],
      });
      const result = isFilterGroupMet(filterGroup, [], card1, reader);
      expect(result).toBeTruthy();
    });
    test('should return true with or operation and 2 filterCause, one is false ', () => {
      const filterClauseNotIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'notIncludes',
        values: ['Status'],
      });
      const filterGroup = createFilterGroup({
        operation: 'or',
        filters: [filterClauseNotIncludes, filterClause],
      });
      const result = isFilterGroupMet(filterGroup, [], card1, reader);
      expect(result).toBeTruthy();
    });
    test('should return true with or operation and 2 filterCause, 1 filtergroup in filtergroup, one filterClause is false ', () => {
      const filterClauseNotIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'notIncludes',
        values: ['Status'],
      });
      const filterGroupInFilterGroup = createFilterGroup({
        operation: 'or',
        filters: [filterClauseNotIncludes, filterClause],
      });
      const filterGroup = createFilterGroup({
        operation: 'or',
        filters: [],
      });
      filterGroup.filters.push(filterGroupInFilterGroup);
      const result = isFilterGroupMet(filterGroup, [], card1, reader);
      expect(result).toBeTruthy();
    });
    test('should return false with or operation and two filterCause, two are false ', () => {
      const filterClauseNotIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'notIncludes',
        values: ['Status'],
      });
      const filterClauseEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'isEmpty',
        values: ['Status'],
      });
      const filterGroup = createFilterGroup({
        operation: 'or',
        filters: [filterClauseNotIncludes, filterClauseEmpty],
      });
      const result = isFilterGroupMet(filterGroup, [], card1, reader);
      expect(result).toBeFalsy();
    });
    test('should return false with and operation and 2 filterCause, one is false ', () => {
      const filterClauseNotIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'notIncludes',
        values: ['Status'],
      });
      const filterGroup = createFilterGroup({
        operation: 'and',
        filters: [filterClauseNotIncludes, filterClause],
      });
      const result = isFilterGroupMet(filterGroup, [], card1, reader);
      expect(result).toBeFalsy();
    });
    test('should return true with and operation and 2 filterCause, two are true ', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'includes',
        values: ['Status'],
      });
      const filterGroup = createFilterGroup({
        operation: 'and',
        filters: [filterClauseIncludes, filterClause],
      });
      const result = isFilterGroupMet(filterGroup, [], card1, reader);
      expect(result).toBeTruthy();
    });
    test('should return true with or operation and 2 filterCause, 1 filtergroup in filtergroup, one filterClause is false ', () => {
      const filterClauseNotIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'notIncludes',
        values: ['Status'],
      });
      const filterGroupInFilterGroup = createFilterGroup({
        operation: 'and',
        filters: [filterClauseNotIncludes, filterClause],
      });
      const filterGroup = createFilterGroup({
        operation: 'and',
        filters: [],
      });
      filterGroup.filters.push(filterGroupInFilterGroup);
      const result = isFilterGroupMet(filterGroup, [], card1, reader);
      expect(result).toBeFalsy();
    });
  });
  describe('verify propertyThatMeetsFilterClause method', () => {
    test('should return Utils.assertFailure and filterClause propertyId ', () => {
      const filterClauseIsNotEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'isNotEmpty',
        values: ['Status'],
      });
      const result = propertyThatMeetsFilterClause(filterClauseIsNotEmpty, []);
      expect(report).toHaveBeenCalledTimes(1);
      expect(result.id).toEqual(filterClauseIsNotEmpty.propertyId);
    });
    test('should return filterClause propertyId with non-select template and isNotEmpty clause ', () => {
      const filterClauseIsNotEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'isNotEmpty',
        values: ['Status'],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIsNotEmpty.propertyId,
        name: 'template',
        type: 'text',
        options: [],
      };
      const result = propertyThatMeetsFilterClause(filterClauseIsNotEmpty, [templateFilter]);
      expect(result.id).toEqual(filterClauseIsNotEmpty.propertyId);
      expect(result.value).toBeFalsy();
    });
    test('should return filterClause propertyId with select template , an option and isNotEmpty clause ', () => {
      const filterClauseIsNotEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'isNotEmpty',
        values: ['Status'],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIsNotEmpty.propertyId,
        name: 'template',
        type: 'select',
        options: [
          {
            id: 'idOption',
            label: '',
            color: 'default',
          },
        ],
      };
      const result = propertyThatMeetsFilterClause(filterClauseIsNotEmpty, [templateFilter]);
      expect(result.id).toEqual(filterClauseIsNotEmpty.propertyId);
      expect(result.value).toEqual('idOption');
    });
    test('should return filterClause propertyId with select template , no option and isNotEmpty clause ', () => {
      const filterClauseIsNotEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'isNotEmpty',
        values: ['Status'],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIsNotEmpty.propertyId,
        name: 'template',
        type: 'select',
        options: [],
      };
      const result = propertyThatMeetsFilterClause(filterClauseIsNotEmpty, [templateFilter]);
      expect(result.id).toEqual(filterClauseIsNotEmpty.propertyId);
      expect(result.value).toBeFalsy();
    });

    test('should return filterClause propertyId with template, and includes clause with values', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'includes',
        values: ['Status'],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIncludes.propertyId,
        name: 'template',
        type: 'text',
        options: [],
      };
      const result = propertyThatMeetsFilterClause(filterClauseIncludes, [templateFilter]);
      expect(result.id).toEqual(filterClauseIncludes.propertyId);
      expect(result.value).toEqual(filterClauseIncludes.values[0]);
    });
    test('should return filterClause propertyId with template, and includes clause with no values', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'includes',
        values: [],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIncludes.propertyId,
        name: 'template',
        type: 'text',
        options: [],
      };
      const result = propertyThatMeetsFilterClause(filterClauseIncludes, [templateFilter]);
      expect(result.id).toEqual(filterClauseIncludes.propertyId);
      expect(result.value).toBeFalsy();
    });
    test('should return filterClause propertyId with template, and notIncludes clause', () => {
      const filterClauseNotIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'notIncludes',
        values: [],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseNotIncludes.propertyId,
        name: 'template',
        type: 'text',
        options: [],
      };
      const result = propertyThatMeetsFilterClause(filterClauseNotIncludes, [templateFilter]);
      expect(result.id).toEqual(filterClauseNotIncludes.propertyId);
      expect(result.value).toBeFalsy();
    });
    test('should return filterClause propertyId with template, and isEmpty clause', () => {
      const filterClauseIsEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'isEmpty',
        values: [],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIsEmpty.propertyId,
        name: 'template',
        type: 'text',
        options: [],
      };
      const result = propertyThatMeetsFilterClause(filterClauseIsEmpty, [templateFilter]);
      expect(result.id).toEqual(filterClauseIsEmpty.propertyId);
      expect(result.value).toBeFalsy();
    });
  });
  describe('verify propertyThatMeetsFilterClause method - Person properties', () => {
    test('should return filterClause propertyId with template, and isEmpty clause', () => {
      const filterClauseIsEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'is',
        values: [],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIsEmpty.propertyId,
        name: 'template',
        type: 'createdBy',
        options: [],
      };
      const result = propertyThatMeetsFilterClause(filterClauseIsEmpty, [templateFilter]);
      expect(result.id).toEqual(filterClauseIsEmpty.propertyId);
      expect(result.value).toBeFalsy();
    });
    test('should return filterClause propertyId with template, and isEmpty clause', () => {
      const filterClauseIsEmpty = createFilterClause({
        propertyId: 'propertyId',
        condition: 'is',
        values: [],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIsEmpty.propertyId,
        name: 'template',
        type: 'createdBy',
        options: [],
      };
      const result = propertyThatMeetsFilterClause(filterClauseIsEmpty, [templateFilter]);
      expect(result.id).toEqual(filterClauseIsEmpty.propertyId);
      expect(result.value).toBeFalsy();
    });
  });
  describe('verify propertiesThatMeetFilterGroup method', () => {
    test('should return {} with undefined filterGroup', () => {
      const result = propertiesThatMeetFilterGroup(undefined, []);
      expect(result).toEqual({});
    });
    test('should return {} with filterGroup without filter', () => {
      const filterGroup = createFilterGroup({
        operation: 'and',
        filters: [],
      });
      const result = propertiesThatMeetFilterGroup(filterGroup, []);
      expect(result).toEqual({});
    });
    test('should return {} with filterGroup, or operation and no template', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'includes',
        values: ['Status'],
      });
      const filterGroup = createFilterGroup({
        operation: 'or',
        filters: [filterClauseIncludes, filterClause],
      });
      const result = propertiesThatMeetFilterGroup(filterGroup, []);
      expect(result).toEqual({});
    });
    test('should return a result with filterGroup, or operation and template', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'includes',
        values: ['Status'],
      });
      const filterGroup = createFilterGroup({
        operation: 'or',
        filters: [filterClauseIncludes, filterClause],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIncludes.propertyId,
        name: 'template',
        type: 'text',
        options: [],
      };
      const result = propertiesThatMeetFilterGroup(filterGroup, [templateFilter]);
      expect(result).toBeDefined();
      expect(result.propertyId).toEqual(filterClauseIncludes.values[0]);
    });
    test('should return {} with filterGroup, and operation and no template', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'includes',
        values: ['Status'],
      });
      const filterGroup = createFilterGroup({
        operation: 'and',
        filters: [filterClauseIncludes, filterClause],
      });
      const result = propertiesThatMeetFilterGroup(filterGroup, []);
      expect(result).toEqual({});
    });

    test('should return a result with filterGroup, and operation and template', () => {
      const filterClauseIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'includes',
        values: ['Status'],
      });
      const filterGroup = createFilterGroup({
        operation: 'and',
        filters: [filterClauseIncludes, filterClause],
      });
      const templateFilter: PropertyTemplate = {
        id: filterClauseIncludes.propertyId,
        name: 'template',
        type: 'text',
        options: [],
      };
      const result = propertiesThatMeetFilterGroup(filterGroup, [templateFilter]);
      expect(result).toBeDefined();
      expect(result.propertyId).toEqual(filterClauseIncludes.values[0]);
    });

    test('looks into the groups of the filter: every part of an "and", the first of an "or"', () => {
      const templates: PropertyTemplate[] = [
        { id: 'status', name: 'Status', type: 'select', options: [] },
        { id: 'phase', name: 'Phase', type: 'select', options: [] },
        { id: 'kind', name: 'Kind', type: 'select', options: [] },
      ];
      const includes = (propertyId: string, value: string) =>
        createFilterClause({ propertyId, condition: 'includes', values: [value] });
      // A group the editor makes: the same condition, one level down.
      const grouped = createFilterGroup({
        operation: 'and',
        filters: [createFilterGroup({ operation: 'or', filters: [includes('status', 'todo')] })],
      });
      expect(propertiesThatMeetFilterGroup(grouped, templates)).toEqual({ status: 'todo' });
      const both = createFilterGroup({
        operation: 'and',
        filters: [
          includes('status', 'todo'),
          createFilterGroup({ operation: 'and', filters: [includes('phase', 'build')] }),
        ],
      });
      expect(propertiesThatMeetFilterGroup(both, templates)).toEqual({
        status: 'todo',
        phase: 'build',
      });
      // Of an "or" the first part alone, a group or not: enough to meet it.
      const either = createFilterGroup({
        operation: 'or',
        filters: [
          createFilterGroup({
            operation: 'and',
            filters: [includes('status', 'todo'), includes('phase', 'build')],
          }),
          includes('kind', 'bug'),
        ],
      });
      expect(propertiesThatMeetFilterGroup(either, templates)).toEqual({
        status: 'todo',
        phase: 'build',
      });
    });
  });
  describe('verify applyFilterGroup method', () => {
    test('should return array with card1', () => {
      const filterClauseNotIncludes = createFilterClause({
        propertyId: 'propertyId',
        condition: 'notIncludes',
        values: ['Status'],
      });
      const filterGroup = createFilterGroup({
        operation: 'or',
        filters: [filterClauseNotIncludes, filterClause],
      });
      const result = applyFilterGroup(filterGroup, [], [card1], reader);
      expect(result).toBeDefined();
      expect(result[0]).toEqual(card1);
    });
  });
  describe('verfiy applyFilterGroup method for case-sensitive search', () => {
    test('should return array with card1 when search by test as Card1', () => {
      const filterClauseNotContains = createFilterClause({
        propertyId: 'title',
        condition: 'contains',
        values: ['Card1'],
      });
      const filterGroup = createFilterGroup({
        operation: 'and',
        filters: [filterClauseNotContains],
      });
      const result = applyFilterGroup(filterGroup, [], [card1], reader);
      expect(result.length).toEqual(1);
    });
  });
  describe('verify applyFilter for title', () => {
    test('should not return array with card1', () => {
      const filterClauseNotContains = createFilterClause({
        propertyId: 'title',
        condition: 'notContains',
        values: ['card1'],
      });
      const filterGroup = createFilterGroup({
        operation: 'and',
        filters: [filterClauseNotContains],
      });
      const result = applyFilterGroup(filterGroup, [], [card1], reader);
      expect(result.length).toEqual(0);
    });
  });
});
