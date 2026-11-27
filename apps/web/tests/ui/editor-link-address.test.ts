/**
 * The address of a link as a link takes it (link-address.ts): a web address
 * or mail without a scheme, a host and its port, and a scheme kept to be
 * refused when a link may not have it.
 */
import { describe, expect, it } from 'vitest';

import { withScheme } from '$lib/editor/link-address.ts';
import { isAllowedHref } from '$lib/editor/schema.ts';

describe('the address of a link', () => {
  it.each([
    ['example.com', 'https://example.com'],
    ['example.com/a?b#c', 'https://example.com/a?b#c'],
    ['localhost:3000', 'https://localhost:3000'],
    ['example.com:8080/x', 'https://example.com:8080/x'],
    ['127.0.0.1:5173', 'https://127.0.0.1:5173'],
    ['me@example.com', 'mailto:me@example.com'],
    ['http://example.com', 'http://example.com'],
    ['mailto:me@example.com', 'mailto:me@example.com'],
    ['javascript:alert(1)', 'javascript:alert(1)'],
    ['ftp://example.com', 'ftp://example.com'],
  ])('%s', (typed, href) => {
    expect(withScheme(typed)).toBe(href);
  });

  it('keeps a scheme a link may not have, to be refused', () => {
    expect(isAllowedHref(withScheme('javascript:alert(1)'))).toBe(false);
    expect(isAllowedHref(withScheme('data:text/html,x'))).toBe(false);
    expect(isAllowedHref(withScheme('localhost:3000'))).toBe(true);
  });
});
