/** Password recovery over HTTP: secrets belong in bodies, never in API addresses. */
import type { PasswordRecovery } from '../state/password-recovery.ts';
import type { Schemas } from './convert.ts';
import type { Http } from './http.ts';

export class ServerPasswordRecovery implements PasswordRecovery {
  readonly #http: Http;

  constructor(http: Http) {
    this.#http = http;
  }

  async available(): Promise<boolean> {
    const options = await this.#http.request<Schemas['LoginOptions']>('/api/v1/login-options', {});
    return options.mailEnabled;
  }

  async request(email: string, language: string): Promise<void> {
    const json: Schemas['RequestPasswordReset'] = { email, language };
    await this.#http.request('/api/v1/password-reset/request', {}, { method: 'POST', json });
  }

  async complete(token: string, password: string): Promise<void> {
    const json: Schemas['FinishPasswordReset'] = { token, password };
    await this.#http.request('/api/v1/password-reset/complete', {}, { method: 'POST', json });
  }
}
