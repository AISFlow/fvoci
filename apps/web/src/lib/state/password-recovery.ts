/** Password recovery without a signed-in account. */
export interface PasswordRecovery {
  /** Whether email recovery is available. */
  available(): Promise<boolean>;
  /** Queues mail without disclosing whether the account exists. */
  request(email: string, language: string): Promise<void>;
  /** Consumes a one-hour link and ends all of the account's sessions. */
  complete(token: string, password: string): Promise<void>;
}
