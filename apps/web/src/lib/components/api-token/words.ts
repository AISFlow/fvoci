/**
 * How a personal API token is told wherever it is listed, in the settings of its owner
 * and on the screen of an organization it reaches: what it may do, and when it was last
 * used, from where if the reader may know.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

/** What a token may do. */
export function scopeLabel(scope: 'read' | 'write'): string {
  return scope === 'read' ? m.account_api_tokens_read() : m.account_api_tokens_write();
}

/** When, and from where if `lastUsedAddress` says, a token was last used, by `dates`; or that it was not. */
export function lastUsedOf(
  token: { lastUsedAt?: number | undefined; lastUsedAddress?: string | undefined },
  dates: Intl.DateTimeFormat,
): string {
  if (token.lastUsedAt === undefined) return m.account_api_tokens_unused();
  const date = dates.format(token.lastUsedAt);
  return token.lastUsedAddress
    ? m.account_api_tokens_used_from({ date, address: token.lastUsedAddress })
    : m.account_api_tokens_used({ date });
}
