/**
 * The page that asks for a password reset link, and the address it is filled
 * in with: in the fragment (`/forgot-password#email=<address>`), which no
 * request carries to the server or to a log.
 */

/** The page under `home` (the app's root), filled in with `email` if any. */
export function forgotPasswordHref(home: string, email: string | undefined): string {
  const page = `${home.replace(/\/$/, '')}/forgot-password`;
  return email ? `${page}#${new URLSearchParams({ email }).toString()}` : page;
}

/** The address the page was opened with, from its fragment (`hash`); empty for none. */
export function emailOfForgotPassword(hash: string): string {
  return new URLSearchParams(hash.replace(/^#/, '')).get('email') ?? '';
}
