/** The reader's own account in the world in memory, changed as the server changes it. */
import {
  CREDENTIALS,
  emptyName,
  MAX_CREDENTIALS,
  NAME_SHOWN,
  shownName,
  TITLES,
  type NameChange,
  type PersonName,
} from '../../logic/person-names.ts';
import { DEFAULT_ACCOUNT_SETTINGS } from '../../logic/account-settings.ts';
import { readDisplaySettings } from '../../logic/display.ts';
import { readLanguageSettings } from '../../logic/language.ts';
import { readNotifySettings } from '../../logic/notify-settings.ts';
import { isTimeZone, readRegionalSettings } from '../../logic/regional.ts';
import { readDisplayName } from '../../logic/users.ts';
import type { AccountSettings, AccountSettingsPatch, User } from '../backend.ts';

import { fail } from './refuse.ts';
import type { World } from './world.ts';

/** The members that are parts of a name, read as a display name is. */
const PARTS = ['firstName', 'lastName', 'latinFirstName', 'latinLastName', 'customName'] as const;

/** A part of a name as the server reads it, or its refusal at the member. */
function part(member: string, value: unknown): string {
  if (value === null) return '';
  if (typeof value !== 'string') {
    fail(400, `${member} must be a string or null`, { field: member, reason: 'invalid' });
  }
  const read = readDisplayName(value);
  if ('refused' in read) fail(400, `${member} is refused`, { field: member, reason: read.refused });
  return read.name;
}

function credentials(value: unknown): PersonName['credentials'] {
  if (value === null) return [];
  if (!Array.isArray(value)) {
    fail(400, 'credentials must be a list', { field: 'credentials', reason: 'invalid' });
  }
  if (value.length > MAX_CREDENTIALS) {
    fail(400, 'at most three credentials', { field: 'credentials', reason: 'too_long' });
  }
  const read: PersonName['credentials'] = [];
  for (const [index, code] of value.entries()) {
    const known = CREDENTIALS.find((credential) => credential === code);
    if (!known || read.includes(known)) {
      fail(400, 'a credential is a code of its list, once', {
        field: String(index),
        within: 'credentials',
        reason: 'invalid',
      });
    }
    read.push(known);
  }
  return read;
}

/**
 * Changes what the patch names of a user's name as the server does
 * (`patchMe`): a part without the white space around it, empty or `null` for
 * none; a code off its list refused at its member, with the server's reason.
 * `displayName` is made anew from the name. Answers the user as stored. Unlike
 * the server, which names every refused member, it names the first alone.
 */
export function changeName(world: World, userId: string, patch: NameChange): User {
  const user = world.users.get(userId) ?? fail(401, 'no session');
  const name: PersonName = { ...emptyName(), ...user.name };
  if (!user.name) name.customName = user.displayName;
  const before = JSON.stringify(name);
  for (const [member, value] of Object.entries(patch) as [string, unknown][]) {
    const asPart = PARTS.find((known) => known === member);
    // Left out of the JSON the server is sent, and so left as it is; so is a choice of `null`.
    if (value === undefined || (member === 'nameShown' && value === null)) continue;
    if (asPart) name[asPart] = part(member, value);
    else if (member === 'nameShown') {
      name.nameShown =
        NAME_SHOWN.find((shown) => shown === value) ??
        fail(400, 'nameShown is a code of its list', { field: member, reason: 'invalid' });
    } else if (member === 'title') {
      name.title =
        value === null
          ? null
          : (TITLES.find((title) => title === value) ??
            fail(400, 'title is a code of its list', { field: member, reason: 'invalid' }));
    } else if (member === 'credentials') name.credentials = credentials(value);
    else fail(400, `there is no member ${member}`, { field: member, reason: 'unknown' });
  }
  // As the server: the same name again is no change, writes nothing and tells nothing.
  if (JSON.stringify(name) === before) return user;
  user.name = name;
  user.displayName = shownName(name);
  user.updatedAt = world.now();
  // Told to everybody here; each reads the person again, and the backend's rule of who
  // sees whom answers (the server tells only who sees them).
  world.tellUsers([...world.users.keys()], { kind: 'user', userId });
  return user;
}

/** The user's settings: the defaults until they change one. */
export function settingsOf(world: World, userId: string): AccountSettings {
  return world.settings.get(userId) ?? { ...DEFAULT_ACCOUNT_SETTINGS };
}

/**
 * The user's settings with what the patch names changed, as the server reads
 * it (`patchSettings`): `null` for the default, a name or a value it does not
 * know refused at its member. Not kept: `keepSettings` keeps and tells them.
 */
export function changedSettings(
  world: World,
  userId: string,
  patch: AccountSettingsPatch,
): AccountSettings {
  const defaults = DEFAULT_ACCOUNT_SETTINGS;
  const merged: Record<string, unknown> = { ...settingsOf(world, userId) };
  for (const [name, value] of Object.entries(patch)) {
    if (!(name in defaults)) fail(400, `unknown setting ${name}`, { field: name });
    merged[name] = value ?? defaults[name as keyof AccountSettings];
  }
  const zone = merged.timeZone;
  if (typeof zone !== 'string' || (zone !== 'auto' && !isTimeZone(zone))) {
    fail(400, 'unknown time zone', { field: 'timeZone' });
  }
  const next: AccountSettings = {
    ...readRegionalSettings(merged),
    ...readDisplaySettings(merged),
    ...readLanguageSettings(merged),
    ...readNotifySettings(merged),
  };
  // A value the reader functions fell back on was not one of the allowed ones.
  for (const name of Object.keys(patch) as (keyof AccountSettings)[]) {
    if (patch[name] !== null && patch[name] !== undefined && next[name] !== patch[name]) {
      fail(400, `invalid value for ${name}`, { field: name });
    }
  }
  return next;
}

/**
 * Changes the user's settings as `changedSettings` reads the patch, keeps
 * them, and tells the user's pages: the settings as they are now.
 */
export function keepSettings(
  world: World,
  userId: string,
  patch: AccountSettingsPatch,
): AccountSettings {
  const next = changedSettings(world, userId, patch);
  world.settings.set(userId, next);
  world.tellUsers([userId], { kind: 'settings', settings: { ...next } });
  return next;
}
