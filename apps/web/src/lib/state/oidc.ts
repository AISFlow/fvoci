/** An organization's single sign-on settings. Secrets are only ever sent. */
export interface OidcProvider {
  issuer: string;
  clientId: string;
  hasClientSecret: boolean;
  autoJoin: boolean;
  enabled: boolean;
  ssoRequired: boolean;
  /** The current session was made through this enabled provider, as it is saved now. */
  canRequireSso: boolean;
  redirectUri: string;
  createdAt: number;
  updatedAt: number;
  /** Settings saved aside while single sign-on is required, until a sign-in through them. */
  pending: OidcPendingProvider | null;
  /** The provider moved away from, while members may still carry their accounts over from it. */
  former: OidcFormerProvider | null;
}

/** The provider before a move to another issuer: nobody signs in through it. */
export interface OidcFormerProvider {
  issuer: string;
  createdAt: number;
  expiresAt: number;
}

/** Pending sign-in settings: nobody signs in through them until a trial succeeds. */
export interface OidcPendingProvider {
  issuer: string;
  clientId: string;
  hasClientSecret: boolean;
  createdAt: number;
  /**
   * For settings of another issuer: until when a sign-in through them from this
   * session, back by then, also connects an identity there that is nobody's yet to
   * the reader's account (five minutes after the session was made through the
   * current provider). Null once that has passed by the server's clock, for a
   * session that was not made so, and for settings of the same issuer.
   */
  bindsIdentityUntil: number | null;
}

export interface OidcSettings {
  issuer: string;
  clientId: string;
  /** Omitted to keep the stored secret; empty to remove it. */
  clientSecret?: string;
  autoJoin: boolean;
  enabled: boolean;
  ssoRequired: boolean;
}
