/** Changing the account's address over HTTP: the token goes in the body, never in an API address. */
import type { EmailChange } from '../state/email-change.ts';
import type { Schemas } from './convert.ts';
import type { Http } from './http.ts';

const PATH = '/api/v1/users/me/email-change';

export class ServerEmailChange implements EmailChange {
  readonly #http: Http;

  constructor(http: Http) {
    this.#http = http;
  }

  async request(email: string, password: string, language: string): Promise<void> {
    const json: Schemas['RequestEmailChange'] = { email, password, language };
    await this.#http.request(PATH, {}, { method: 'POST', json });
  }

  async resend(language: string): Promise<void> {
    const json: Schemas['ResendEmailChange'] = { language };
    await this.#http.request(`${PATH}/resend`, {}, { method: 'POST', json });
  }

  async cancel(): Promise<void> {
    await this.#http.request(PATH, {}, { method: 'DELETE' });
  }

  async confirm(token: string): Promise<void> {
    const json: Schemas['ConfirmEmailChange'] = { token };
    await this.#http.request('/api/v1/email-change/confirm', {}, { method: 'POST', json });
  }

  async revert(token: string): Promise<string | undefined> {
    const json: Schemas['RevertEmailChange'] = { token };
    const answer = await this.#http.request<Schemas['RevertedEmailChange']>(
      '/api/v1/email-change/revert',
      {},
      { method: 'POST', json },
    );
    return answer.email ?? undefined;
  }
}
