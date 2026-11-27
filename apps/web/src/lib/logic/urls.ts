/** Addresses inside the app. Nothing here reads `window`: the caller passes what is needed. */
import type { Container } from './wire.ts';

/** A board or a wiki (`kind`) as far as its address goes: itself, and the workspace it is of. */
export type ContainerPlace = Pick<Container, 'id' | 'workspaceId' | 'kind'>;

/** The base path as the server gives it, without slashes at either end. */
export function normalizeBasePath(raw: string | undefined): string {
  const trimmed = (raw ?? '').replace(/\/+$/, '');
  return trimmed.startsWith('/') ? trimmed.slice(1) : trimmed;
}

/**
 * The address of `path` under the base path: `/base/path`, always from the
 * root of the site. With `origin` the result is absolute.
 */
export function buildUrl(path: string, basePath: string | undefined, origin?: string): string {
  const base = normalizeBasePath(basePath);
  const relative = path.startsWith('/') ? path.slice(1) : path;
  const joined = base === '' ? `/${relative}` : `/${base}/${relative}`;
  return origin === undefined ? joined : `${origin}${joined}`;
}

/**
 * Fills the parameters into a route pattern such as
 * `/workspace/:workspaceId/:boardId?/:viewId?`. A missing optional parameter (`?`)
 * drops its segment; a missing required one is an error.
 */
export function fillPath(
  pattern: string,
  params: Readonly<Record<string, string | undefined>>,
): string {
  const segments: string[] = [];
  for (const segment of pattern.split('/')) {
    if (!segment.startsWith(':')) {
      segments.push(segment);
      continue;
    }
    const optional = segment.endsWith('?');
    const name = segment.slice(1, optional ? -1 : undefined);
    const value = params[name];
    if (value) segments.push(value);
    else if (!optional) throw new Error(`missing route parameter ${name}`);
  }
  return segments.join('/');
}

/**
 * The address that shows `boardId` from the current route. The same board
 * keeps the open view and card; another board drops them, so that its first
 * view is shown.
 */
export function boardPath(
  pattern: string,
  params: Readonly<Record<string, string | undefined>>,
  boardId: string,
): string {
  const next: Record<string, string | undefined> = { ...params, boardId };
  if (boardId !== params.boardId) {
    next.viewId = undefined;
    next.cardId = undefined;
  }
  return fillPath(pattern, next);
}
