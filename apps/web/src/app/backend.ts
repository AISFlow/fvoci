/**
 * Which backend the app runs on. This is the one file that picks an
 * implementation of the data boundary (`lib/state/backend.ts`); everything
 * else sees the interface only, and a lint rule keeps it so.
 *
 * Two are there: the server's API (`lib/api/`), and one in memory, seeded
 * with the built-in templates, for looking at the screens without a server.
 * The page says which (`chosenBackend`). Each is fetched as a chunk of its
 * own, so the app the server serves does not carry the one in memory.
 */
import { getLocale } from '$lib/i18n/paraglide/runtime.js';
import type { Account, Backend, People } from '$lib/state/index.ts';
import type { EmailChange } from '$lib/state/email-change.ts';
import type { EmailVerification, WaitingAccount } from '$lib/state/email-verification.ts';
import type { PasswordRecovery } from '$lib/state/password-recovery.ts';
import type { Organizations } from '$lib/state/organization.ts';

import { readBasePath } from './router.svelte.ts';
import { display } from '$lib/display.svelte.ts';
import { DEFAULT_ACCOUNT_SETTINGS } from '$lib/logic/account-settings.ts';
import { readDisplaySettings } from '$lib/logic/display.ts';
import { readLanguageSettings } from '$lib/logic/language.ts';
import { readNotifySettings } from '$lib/logic/notify-settings.ts';
import { readRegionalSettings } from '$lib/logic/regional.ts';

export interface StartedBackend {
  backend: Backend;
  /** Signing in and out; only a backend with accounts has it. */
  account?: Account;
  /** Managing who is in a workspace; only a backend with accounts has it. */
  people?: People;
  /** The organization's administration and the workspaces' records; only the server has them. */
  organizations?: Organizations;
}

export type BackendName = 'server' | 'memory';

/**
 * The backend the page asks for in `<meta name="fvoci:backend">`. The
 * development aids that serve the app without a server (tools/preview.ts,
 * tools/serve.ts) write `memory` there; the server writes nothing, and a
 * built app then calls it. The development server of Vite serves the
 * source, without the tag, and runs in memory.
 */
export function chosenBackend(document: Document): BackendName {
  const asked = document.querySelector<HTMLMetaElement>('meta[name="fvoci:backend"]')?.content;
  if (asked === 'memory' || asked === 'server') return asked;
  return import.meta.env.DEV ? 'memory' : 'server';
}

/** Where the in-memory backend keeps the reader's settings between visits. */
const SETTINGS_KEY = 'fvoci:memory-settings';

/**
 * A world in memory starts from its seed on every visit, and with it the
 * reader's account settings. So that a theme or a time zone chosen in
 * the settings is still there after a reload, as it would be with a server,
 * the settings are kept on the device and given back to the new world.
 */
async function keepSettings(backend: Backend): Promise<void> {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? 'null');
    // Only the settings this version knows, each read as it would be from
    // the server: a name it no longer has (one saved by an older version) is
    // left out, not refused with all the others.
    if (saved)
      await backend.patchSettings({
        ...readRegionalSettings(saved),
        ...readDisplaySettings(saved),
        ...readLanguageSettings(saved),
        ...readNotifySettings(saved),
      });
  } catch {
    // Nothing kept, or something the backend refuses now: the defaults then.
  }
  backend.watchSelf((change) => {
    if (change.kind !== 'settings') return;
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(change.settings));
    } catch {
      // Without storage the settings last for the visit.
    }
  });
}

/** Where the in-memory backend keeps the state the client keeps with the account between visits. */
const SCREEN_STATE_KEY = 'fvoci:memory-screen-state';

/**
 * As the settings (`keepSettings`), the state the client keeps with the
 * account (whether the time zone follows the device, a notice closed for
 * good) is still there after a reload, as with a server: it is kept on the
 * device as the page goes (`current` reads it then), and given back to the
 * new world.
 */
async function keepScreenState(
  backend: Backend,
  current: () => Record<string, string> | undefined,
): Promise<void> {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(SCREEN_STATE_KEY) ?? 'null');
    if (saved !== null && typeof saved === 'object') {
      const entries = Object.entries(saved).filter(
        (entry): entry is [string, string] => typeof entry[1] === 'string',
      );
      await backend.patchScreenState(Object.fromEntries(entries));
    }
  } catch {
    // Nothing kept, or something the backend refuses now: none then.
  }
  addEventListener('pagehide', () => {
    try {
      localStorage.setItem(SCREEN_STATE_KEY, JSON.stringify(current() ?? {}));
    } catch {
      // Without storage the state lasts for the visit.
    }
  });
}

async function startServer(): Promise<StartedBackend> {
  const [{ ServerBackend }, { Http }, { ServerOrganizations }] = await Promise.all([
    import('$lib/api/backend.ts'),
    import('$lib/api/http.ts'),
    import('$lib/api/organizations.ts'),
  ]);
  const base = readBasePath(document);
  const path = base === '' ? '' : `/${base}`;
  const http = new Http(path);
  const backend = new ServerBackend(path, http);
  return {
    backend,
    account: backend,
    people: backend.people,
    organizations: new ServerOrganizations(http),
  };
}

/**
 * The backend of a reader by a board's or a wiki's public link (`/shared/…`):
 * the server's, which reads it by the link's read token alone, and the place
 * it shows it in.
 */
export async function startShared(
  containerId: string,
  readToken: string,
): Promise<{ backend: Backend; place: string }> {
  const { SharedBackend, SHARED_PLACE } = await import('$lib/api/shared-backend.ts');
  const base = readBasePath(document);
  // The device's theme and density stay as they are: the reader has no account's.
  const settings = { ...DEFAULT_ACCOUNT_SETTINGS, ...display.settings };
  const backend = new SharedBackend(
    base === '' ? '' : `/${base}`,
    containerId,
    readToken,
    settings,
  );
  return { backend, place: SHARED_PLACE };
}

export async function startBackend(): Promise<StartedBackend> {
  return chosenBackend(document) === 'server' ? startServer() : startMemory();
}

async function startMemory(): Promise<StartedBackend> {
  const [{ MemoryBackend }, { loadTemplates, seedWorld }, { eraseUser }, { noteUpcomingMeeting }] =
    await Promise.all([
      import('$lib/state/memory/backend.ts'),
      import('$lib/state/memory/seed.ts'),
      import('$lib/state/memory/users.ts'),
      import('$lib/state/memory/notifications.ts'),
    ]);
  const { world, readerId } = seedWorld(await loadTemplates());
  // The tabs of this browser edit their bodies together, as through one server.
  world.shareRooms = 'fvoci:memory-rooms';
  // `?search=0` is a client configuration that says bodies are not searched: no search box.
  world.searchEnabled = new URLSearchParams(location.search).get('search') !== '0';
  // `?copyOut=0` is an organization that lets nothing be copied out of it (`allowCopyOut` off).
  world.allowCopyOut = new URLSearchParams(location.search).get('copyOut') !== '0';
  // `?held=<ms>` holds what the rooms send that long after a stroke of a syllable (World.heldIdle).
  const held = Number(new URLSearchParams(location.search).get('held') ?? '');
  if (Number.isFinite(held) && held > 0) world.heldIdle = held;
  const otherIds = [...world.users.keys()].filter((id) => id !== readerId);
  const otherId = otherIds[0];
  // `?gone=1` says the other person's account was deleted: what they wrote is a deleted user's.
  const gone = new URLSearchParams(location.search).get('gone') === '1';
  if (gone && otherId !== undefined) eraseUser(world, otherId);
  const backend = new MemoryBackend(world, readerId);
  // Long enough to see that a change shows before it is answered.
  backend.latency = 40;
  await keepSettings(backend);
  await keepScreenState(backend, () => world.screenState.get(readerId));
  const asSomeoneElse =
    otherId === undefined ? undefined : (index = 0) => backend.as(otherIds[index] ?? otherId);
  // A handle for the browser tests and for trying things by hand in the
  // console: the same data as another person, and the next call failing.
  (window as unknown as { fvociDev?: unknown }).fvociDev = {
    backend,
    someoneElse: asSomeoneElse,
    notifyUpcomingMeeting: (courseId: string, meetingId: string) => {
      noteUpcomingMeeting(world, courseId, meetingId);
    },
    eraseUser: (userId: string) => {
      eraseUser(world, userId);
    },
    // The formula library as the bodies draw with it, for e2e/editor-attacks.spec.ts.
    math: () => import('$lib/editor/math.ts'),
  };
  return { backend };
}

/** Confirming a registration's address is available before there is an account. */
export async function startEmailVerification(): Promise<EmailVerification> {
  const [{ ServerEmailVerification }, { Http }] = await Promise.all([
    import('$lib/api/email-verification.ts'),
    import('$lib/api/http.ts'),
  ]);
  const base = readBasePath(document);
  return new ServerEmailVerification(new Http(base === '' ? '' : `/${base}`));
}

/** Mails a registration that waits for its address a new link, in the page's language. */
export async function resendLink(waiting: WaitingAccount): Promise<void> {
  await (await startEmailVerification()).resend(waiting, getLocale());
}

/** The link that changes an account's address is opened signed in or not. */
export async function startEmailChange(): Promise<EmailChange> {
  const [{ ServerEmailChange }, { Http }] = await Promise.all([
    import('$lib/api/email-change.ts'),
    import('$lib/api/http.ts'),
  ]);
  const base = readBasePath(document);
  return new ServerEmailChange(new Http(base === '' ? '' : `/${base}`));
}

/** Recovery is available before there is an account or a workspace to load. */
export async function startPasswordRecovery(): Promise<PasswordRecovery> {
  const [{ ServerPasswordRecovery }, { Http }] = await Promise.all([
    import('$lib/api/password-recovery.ts'),
    import('$lib/api/http.ts'),
  ]);
  const base = readBasePath(document);
  return new ServerPasswordRecovery(new Http(base === '' ? '' : `/${base}`));
}
