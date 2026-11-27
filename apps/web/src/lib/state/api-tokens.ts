/**
 * The reader's personal API tokens, which they give an agent to work in
 * their workspaces as them: read-only, or reading and writing content. The
 * token itself is told once, when it is made; the list shows its first
 * characters.
 */

/** What a token may do. */
export type ApiTokenScope = 'read' | 'write';

/** A workspace a token reaches. */
export interface ApiTokenWorkspace {
  workspaceId: string;
  orgId: string;
  name: string;
}

/** A token as its list shows it. */
export interface ApiToken {
  id: string;
  userId: string;
  name: string;
  /** `fvoci_pat_` and six more characters: which token a client holds. */
  tokenPrefix: string;
  scope: ApiTokenScope;
  workspaces: ApiTokenWorkspace[];
  allowPersonalSpace: boolean;
  /** Whether its owner is told of what it does, as of anybody's doing (not in their own space). */
  notifySelf: boolean;
  createdAt: number;
  expiresAt: number;
  /** When it was last used, to within five minutes; `undefined` if never. */
  lastUsedAt?: number | undefined;
  /** The address it was last used from; `undefined` if never, or unknown. */
  lastUsedAddress?: string | undefined;
}

/**
 * A token as an administrator of an organization it reaches sees it: whose it is, what
 * it may do there and when it was made, last used and ends. Where else it reaches,
 * where it was used from and what its owner asked of it are its owner's alone.
 */
export type MemberApiToken = Omit<
  ApiToken,
  'allowPersonalSpace' | 'notifySelf' | 'lastUsedAddress'
>;

/** What a new token is asked to be. */
export interface NewApiToken {
  name: string;
  scope: ApiTokenScope;
  workspaceIds: string[];
  expiresInDays: number;
  /** Whether it reaches the reader's own space too, which `password` gives. */
  allowPersonalSpace: boolean;
  password?: string | undefined;
  /** Whether the reader is told of what it does. */
  notifySelf: boolean;
}

/** A token as it is made: the only time `token` is told. */
export interface IssuedApiToken {
  token: string;
  apiToken: ApiToken;
}

export interface ApiTokens {
  /** The reader's tokens that still open something, the newest first. */
  list(): Promise<ApiToken[]>;
  /**
   * Makes one. 400 at the member a request gets wrong; 403 `sign_in_again`
   * or 400 `invalid_credentials` for the own space without the password.
   */
  issue(token: NewApiToken): Promise<IssuedApiToken>;
  /** Ends one at once; 404 for one that is gone. */
  revoke(tokenId: string): Promise<void>;
}

/** The tokens of an organization's members that reach it, as its administrator sees them. */
export interface OrgApiTokens {
  list(orgId: string): Promise<MemberApiToken[]>;
  /**
   * Takes one out of the organization: it reaches none of its workspaces any more, and
   * a token left reaching nothing is ended. 404 for one that reaches none of them.
   */
  revoke(orgId: string, tokenId: string): Promise<void>;
}
