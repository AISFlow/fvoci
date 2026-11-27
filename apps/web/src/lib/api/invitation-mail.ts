/** Optional SMTP delivery for an existing member or guest invitation. */
import type { InvitationMail } from '../state/people-types.ts';
import type { Schemas } from './convert.ts';
import type { Http } from './http.ts';

export class ServerInvitationMail implements InvitationMail {
  readonly #http: Http;
  #available: Promise<boolean> | undefined;

  constructor(http: Http) {
    this.#http = http;
  }

  available(): Promise<boolean> {
    return (this.#available ??= this.#http
      .request<Schemas['LoginOptions']>('/api/v1/login-options', {})
      .then((options) => options.mailEnabled)
      .catch((error: unknown) => {
        // A failed read is asked again next time, rather than hiding mail for good.
        this.#available = undefined;
        throw error;
      }));
  }

  async send(
    workspaceId: string,
    token: string,
    recipients: string[],
    language: string,
  ): Promise<void> {
    const json: Schemas['InvitationMail'] = { token, recipients, language };
    await this.#http.request(
      '/api/v1/w/{workspaceId}/invitations/mail',
      { workspaceId },
      { method: 'POST', json },
    );
  }
}
