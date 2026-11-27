/**
 * The utilities, each against the module it lives in. The functions take the
 * base URL, the origin and the reader's settings as arguments: none of them
 * reads `window`.
 */
import { describe, expect, it, test } from 'vitest';

import { dateOfInstant } from '$lib/logic/dates.ts';
import { displayDate, displayDateTime, inputDate } from '$lib/logic/format.ts';
import { countCheckboxesInMarkdown, ensureProtocol } from '$lib/logic/text.ts';
import { boardPath, buildUrl } from '$lib/logic/urls.ts';
import { compareVersions } from '$lib/logic/versions.ts';
import { createId } from '$lib/logic/wire.ts';

import { at } from '../support/zones.ts';

describe('utils', () => {
  describe('assureProtocol', () => {
    test('should passthrough on valid short protocol', () => {
      expect(ensureProtocol('https://example.com')).toBe('https://example.com');
    });
    test('should passthrough on valid long protocol', () => {
      expect(ensureProtocol('somecustomprotocol://example.com')).toBe(
        'somecustomprotocol://example.com',
      );
    });

    test('should passthrough on valid short protocol', () => {
      expect(ensureProtocol('x://example.com')).toBe('x://example.com');
    });

    test('should add a https for empty protocol', () => {
      expect(ensureProtocol('example.com')).toBe('https://example.com');
    });
  });

  describe('createGuid', () => {
    test('should create 27 char random id for workspace', () => {
      expect(createId('workspace')).toMatch(/^w[ybndrfg8ejkmcpqxot1uwisza345h769]{26}$/);
    });
    test('should create 27 char random id for board', () => {
      expect(createId('board')).toMatch(/^b[ybndrfg8ejkmcpqxot1uwisza345h769]{26}$/);
    });
    test('should create 27 char random id for card', () => {
      expect(createId('card')).toMatch(/^c[ybndrfg8ejkmcpqxot1uwisza345h769]{26}$/);
    });
    test('should create 27 char random id', () => {
      expect(createId('none')).toMatch(/^7[ybndrfg8ejkmcpqxot1uwisza345h769]{26}$/);
    });
  });

  describe('countCheckboxesInMarkdown', () => {
    test('should count checkboxes', () => {
      const text = `
                ## Header
                - [x] one
                - [ ] two
                - [x] three
            `.replace(/\n\s+/gm, '\n');
      const checkboxes = countCheckboxesInMarkdown(text);
      expect(checkboxes.total).toBe(3);
      expect(checkboxes.checked).toBe(2);
    });
  });

  describe('test - buildURL', () => {
    const origin = 'http://localhost';

    test('buildURL, no base', () => {
      expect(buildUrl('test', undefined, origin)).toBe('http://localhost/test');
      expect(buildUrl('/test', undefined, origin)).toBe('http://localhost/test');

      expect(buildUrl('test', undefined)).toBe('/test');
      expect(buildUrl('/test', undefined)).toBe('/test');
    });

    // Q8: the address starts at the root of the site.
    test('buildURL, base no slash', () => {
      expect(buildUrl('test', 'base', origin)).toBe('http://localhost/base/test');
      expect(buildUrl('/test', 'base', origin)).toBe('http://localhost/base/test');

      expect(buildUrl('test', 'base')).toBe('/base/test');
      expect(buildUrl('/test', 'base')).toBe('/base/test');
    });

    // Q8, as above.
    test('buildUrl, base with slash', () => {
      expect(buildUrl('test', '/base/', origin)).toBe('http://localhost/base/test');
      expect(buildUrl('/test', '/base/', origin)).toBe('http://localhost/base/test');

      expect(buildUrl('test', '/base/')).toBe('/base/test');
      expect(buildUrl('/test', '/base/')).toBe('/base/test');
    });
  });

  // The reader is in Seoul, wherever the tests run: no date here depends on
  // the time zone of the machine.
  const timeZone = 'Asia/Seoul';
  const currentYear = Number(dateOfInstant(Date.now(), timeZone).slice(0, 4));
  const previousYear = currentYear - 1;

  describe('display date', () => {
    const reader = { locale: 'en-us', timeZone };

    it('should show month and day for current year', () => {
      const date = at(timeZone, currentYear, 7, 9);
      expect(displayDate(date, reader)).toBe('July 09');
    });

    it('should show month, day and year for previous year', () => {
      const date = at(timeZone, previousYear, 7, 9);
      expect(displayDate(date, reader)).toBe(`July 09, ${previousYear}`);
    });
  });

  describe('input date', () => {
    const date = at(timeZone, currentYear, 7, 9);

    it('should show mm/dd/yyyy for current year', () => {
      expect(inputDate(date, { locale: 'en-us', timeZone })).toBe(`07/09/${currentYear}`);
    });

    it('should show dd/mm/yyyy for current year, es local', () => {
      expect(inputDate(date, { locale: 'es-es', timeZone })).toBe(`09/07/${currentYear}`);
    });
  });

  describe('display date and time', () => {
    const reader = { locale: 'en-us', timeZone };

    it('should show month, day and time for current year', () => {
      const date = at(timeZone, currentYear, 7, 9, 15, 20);
      expect(displayDateTime(date, reader)).toBe('July 09 at 3:20 PM');
    });

    it('should show month, day, year and time for previous year', () => {
      const date = at(timeZone, previousYear, 7, 9, 5, 35);
      expect(displayDateTime(date, reader)).toBe(`July 09, ${previousYear} at 5:35 AM`);
    });
  });

  describe('compare versions', () => {
    it('should return one if b > a', () => {
      expect(compareVersions('0.9.4', '0.10.0')).toBe(1);
    });

    it('should return zero if a = b', () => {
      expect(compareVersions('1.2.3', '1.2.3')).toBe(0);
    });

    it('should return minus one if b < a', () => {
      expect(compareVersions('10.9.4', '10.9.2')).toBe(-1);
    });
  });

  describe('showBoard test', () => {
    it('should switch boards', () => {
      const params = {
        boardId: 'board_id_1',
        viewId: 'view_id_1',
        cardId: 'card_id_1',
        workspaceId: 'workspace_id_1',
      };
      const pattern = '/workspace/:workspaceId/:boardId?/:viewId?/:cardId?';

      expect(boardPath(pattern, params, 'board_id_2')).toBe('/workspace/workspace_id_1/board_id_2');
    });
  });
});
