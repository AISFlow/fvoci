/**
 * What the screen of personal API tokens says and makes: the name a new token
 * is given, the command that connects an agent with it, the workspaces it may
 * be given, and the words of a refusal (how a token listed is told is
 * `$lib/components/api-token/words.ts`). The words come from the refusal's
 * status, reason and the member it points at, never from the server's detail.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError, type WorkspaceInfo } from '$lib/state/index.ts';

/** How many days a new token lasts unless it is asked otherwise. */
export const TOKEN_DAYS = 90;

/** The name a token made on `day` is given at first. */
export function defaultTokenName(day: Date): string {
  return m.account_api_tokens_default_name({
    month: String(day.getMonth() + 1),
    day: String(day.getDate()),
  });
}

/**
 * The command that connects Claude Code to the server with `token`, as an
 * agent's MCP server: the token goes in a header, never in an address.
 */
export function mcpCommand(origin: string, token: string): string {
  return `claude mcp add --transport http fvoci ${origin}/api/v1/mcp --header "Authorization: Bearer ${token}"`;
}

/** A workspace a token may be given: one of an organization the reader works in, not as a guest. */
export interface TokenWorkspace {
  id: string;
  name: string;
}

/** The workspaces of `workspaces` a token may be given, in their order. */
export function tokenWorkspaces(workspaces: readonly WorkspaceInfo[]): TokenWorkspace[] {
  return workspaces
    .filter((one) => one.kind !== 'personal' && one.role !== 'guest')
    .map((one) => ({ id: one.id, name: one.title }));
}

export interface TokenProblem {
  text: string;
  /** The field to point at, if the refusal is about one. */
  field: 'name' | 'password' | undefined;
}

export function tokenProblemOf(error: unknown): TokenProblem {
  if (!isBackendError(error)) return { text: m.account_failed(), field: undefined };
  if (error.reason === 'invalid_credentials') {
    return { text: m.account_password_wrong(), field: 'password' };
  }
  if (error.reason === 'sign_in_again') {
    return { text: m.account_api_tokens_sign_in_again(), field: undefined };
  }
  if (error.reason === 'api_tokens_disallowed') {
    return { text: m.account_api_tokens_disallowed(), field: undefined };
  }
  if (error.reason === 'too_many_api_tokens') {
    return { text: m.account_api_tokens_too_many(), field: undefined };
  }
  // 409: the session stays signed in, and one made through the provider as it is now
  // makes the token.
  if (error.reason === 'provider_changed') {
    return { text: m.account_api_tokens_provider_changed(), field: undefined };
  }
  if (error.status === 429) return { text: m.account_rate_limited(), field: undefined };
  if (error.status === 400 && error.refusalAt('/name')) {
    return { text: m.account_api_tokens_name_invalid(), field: 'name' };
  }
  if (error.status === 400 && error.refusalAt('/workspaceIds')) {
    return { text: m.account_api_tokens_workspaces_invalid(), field: undefined };
  }
  return { text: m.account_failed(), field: undefined };
}
