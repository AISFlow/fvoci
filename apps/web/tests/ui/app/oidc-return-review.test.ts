/** The return path of an SSO challenge includes the organization administration page. */
import { beforeEach, expect, it } from 'vitest';

import { keepReturn, takeReturn } from '../../../src/app/return-to.ts';
import { parseRoute, Router } from '../../../src/app/router.svelte.ts';

beforeEach(() => {
  sessionStorage.clear();
  history.replaceState(null, '', '/');
});

it('returns to the challenged organization after signing in through its provider', () => {
  const destination = '/org/00000000-0000-7000-8000-000000000001';
  keepReturn(destination);
  const router = new Router('');
  const kept = takeReturn();
  expect(kept).toBe(destination);
  if (kept === undefined) throw new Error('the return path was not saved');

  // App.resume passes the saved, parsed route to this same router operation.
  const route = parseRoute(kept, router.basePath);
  expect(route.kind).toBe('org');
  router.go(route, { replace: true });

  expect(router.route).toEqual({ kind: 'org', orgId: '00000000-0000-7000-8000-000000000001' });
  expect(location.pathname).toBe(destination);
});
