/** Confirming a registration's address over HTTP: its tokens go in bodies, never in addresses. */
import type { Confirmed, EmailVerification, WaitingAccount } from '../state/email-verification.ts';
import type { Schemas } from './convert.ts';
import type { Http } from './http.ts';
import { makeWelcomeBoard } from './templates.ts';

export class ServerEmailVerification implements EmailVerification {
  readonly #http: Http;

  constructor(http: Http) {
    this.#http = http;
  }

  async resend(waiting: WaitingAccount, language: string): Promise<void> {
    const json: Schemas['RequestEmailVerification'] = {
      username: waiting.username ?? '',
      email: waiting.email ?? '',
      language,
    };
    await this.#http.request('/api/v1/email-verification/request', {}, { method: 'POST', json });
  }

  async complete(token: string, password?: string): Promise<Confirmed> {
    const json: Schemas['CompleteEmailVerification'] = { token, ...(password ? { password } : {}) };
    const confirmed = await this.#http.request<Schemas['EmailVerified']>(
      '/api/v1/email-verification/complete',
      {},
      { method: 'POST', json },
    );
    return {
      workspaceId: confirmed.workspaceId,
      ...(confirmed.containerId ? { containerId: confirmed.containerId } : {}),
      ...(confirmed.containerKind ? { containerKind: confirmed.containerKind } : {}),
      organizationStarted: confirmed.organizationStarted,
    };
  }

  /** Makes the welcome board of a new organization's first workspace. */
  welcome(workspaceId: string): Promise<string> {
    return makeWelcomeBoard(this.#http, workspaceId);
  }
}
