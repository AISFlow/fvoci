/** Personal API tokens over HTTP: the reader's own, and those an administrator sees. */
import type {
  ApiToken,
  ApiTokens,
  IssuedApiToken,
  MemberApiToken,
  NewApiToken,
  OrgApiTokens,
} from '../state/api-tokens.ts';

import type { Schemas } from './convert.ts';
import type { Http } from './http.ts';

const MINE = '/api/v1/users/me/api-tokens';

function tokenOf(token: Schemas['ApiToken']): ApiToken {
  return {
    id: token.id,
    userId: token.userId,
    name: token.name,
    tokenPrefix: token.tokenPrefix,
    scope: token.scope,
    workspaces: token.workspaces,
    allowPersonalSpace: token.allowPersonalSpace,
    notifySelf: token.notifySelf,
    createdAt: token.createdAt,
    expiresAt: token.expiresAt,
    lastUsedAt: token.lastUsedAt ?? undefined,
    lastUsedAddress: token.lastUsedAddress ?? undefined,
  };
}

function memberTokenOf(token: Schemas['MemberApiToken']): MemberApiToken {
  return {
    id: token.id,
    userId: token.userId,
    name: token.name,
    tokenPrefix: token.tokenPrefix,
    scope: token.scope,
    workspaces: token.workspaces,
    createdAt: token.createdAt,
    expiresAt: token.expiresAt,
    lastUsedAt: token.lastUsedAt ?? undefined,
  };
}

/** The reader's own tokens. */
export class ServerApiTokens implements ApiTokens {
  readonly #http: Http;

  constructor(http: Http) {
    this.#http = http;
  }

  async list(): Promise<ApiToken[]> {
    return (await this.#http.request<Schemas['ApiToken'][]>(MINE)).map(tokenOf);
  }

  async issue(token: NewApiToken): Promise<IssuedApiToken> {
    const json: Schemas['NewApiToken'] = {
      name: token.name,
      scope: token.scope,
      workspaceIds: token.workspaceIds,
      expiresInDays: token.expiresInDays,
      allowPersonalSpace: token.allowPersonalSpace,
      notifySelf: token.notifySelf,
      ...(token.password ? { password: token.password } : {}),
    };
    const issued = await this.#http.request<Schemas['IssuedApiToken']>(
      MINE,
      {},
      { method: 'POST', json },
    );
    return { token: issued.token, apiToken: tokenOf(issued.apiToken) };
  }

  async revoke(tokenId: string): Promise<void> {
    await this.#http.request(
      '/api/v1/users/me/api-tokens/{tokenId}',
      { tokenId },
      {
        method: 'DELETE',
      },
    );
  }
}

/** The tokens of an organization's members, as its administrator sees them. */
export class ServerOrgApiTokens implements OrgApiTokens {
  readonly #http: Http;

  constructor(http: Http) {
    this.#http = http;
  }

  async list(orgId: string): Promise<MemberApiToken[]> {
    const listed = await this.#http.request<Schemas['MemberApiToken'][]>(
      '/api/v1/org/{orgId}/api-tokens',
      { orgId },
    );
    return listed.map(memberTokenOf);
  }

  async revoke(orgId: string, tokenId: string): Promise<void> {
    await this.#http.request(
      '/api/v1/org/{orgId}/api-tokens/{tokenId}',
      { orgId, tokenId },
      { method: 'DELETE' },
    );
  }
}
