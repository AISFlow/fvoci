/**
 * The addresses of the app that name a card or a page, as far as a reference
 * goes: pasted into a body, such an address is what it names.
 *
 * A card's is `/w/<workspace>/boards/<board>/<view>/<card>`, a page's
 * `/w/<workspace>/wikis/<wiki>/<page>`, each under the base path the app is
 * served at. Whether the identifier names anything is not asked here.
 */
import { normalizeBasePath } from './urls.ts';

/** Where the app is, and the workspace the reader is in. */
export interface Here {
  /** The origin of the page: an address of another site names nothing here. */
  origin: string;
  basePath: string;
  /** The workspace entered: what is of another workspace is not named. */
  workspaceId: string | undefined;
}

function decoded(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

/** The identifier of the card or the page that `address` names in the workspace entered. */
export function referenceAt(address: string, here: Here): string | undefined {
  if (here.workspaceId === undefined) return undefined;
  let url: URL;
  try {
    url = new URL(address);
  } catch {
    return undefined;
  }
  if (url.origin !== here.origin) return undefined;
  const base = normalizeBasePath(here.basePath);
  const prefix = base === '' ? '' : `/${base}`;
  if (prefix !== '' && !url.pathname.startsWith(`${prefix}/`)) return undefined;
  const segments = url.pathname
    .slice(prefix.length)
    .split('/')
    .filter((segment) => segment !== '')
    .map(decoded);
  const [w, workspaceId, kind, , first, second, ...rest] = segments;
  if (w !== 'w' || workspaceId !== here.workspaceId || rest.length > 0) return undefined;
  if (kind === 'boards') return second;
  if (kind === 'wikis' || kind === 'courses') return second === undefined ? first : undefined;
  return undefined;
}
