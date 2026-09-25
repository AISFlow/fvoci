import { t, tProblemTitle } from "@fvoci/i18n";
import createClient from "openapi-fetch";
import type { components, paths } from "@/generated/api";
import { CONSENT_PATH, consentUrl, isConsentRequired } from "@/lib/consent";

/**
 * Source 428 branch: a signed-in user with pending required legal documents
 * gets `consent_required` on every gated request. Send them to the prompt with
 * the current page as `returnTo`. The request never settles, so callers do not
 * race the page change with their own error redirects (e.g. me → /login).
 */
export async function consentGate(response: Response): Promise<Response> {
  if (response.status !== 428 || window.location.pathname === CONSENT_PATH) {
    return response;
  }
  const body: unknown = await response
    .clone()
    .json()
    .catch(() => null);
  if (!isConsentRequired(response.status, body)) return response;
  window.location.assign(consentUrl(window.location));
  return new Promise<Response>(() => {});
}

export const api = createClient<paths>({
  credentials: "include",
  fetch: async (input) => consentGate(await globalThis.fetch(input)),
});

type ProblemBody = components["schemas"]["ProblemResponse"];

export class ProblemError extends Error {
  readonly status: number;
  readonly title: string;
  readonly titleKnown: boolean;
  readonly code?: string;

  constructor(status: number, code?: string, params?: Record<string, string | number>) {
    const localized = code ? tProblemTitle(code, params) : t("error.http.fallback");
    super(localized);
    this.name = "ProblemError";
    this.status = status;
    this.title = localized;
    this.titleKnown = Boolean(code);
    this.code = code;
  }
}

export function problemMessage(err: unknown, fallback: Parameters<typeof t>[0]): string {
  if (!(err instanceof ProblemError)) return t("error.network");
  return err.titleKnown ? err.title : t(fallback);
}

type ApiResult<T> = {
  data?: T;
  error?: ProblemBody;
  response: Response;
};

export async function ensureOk<T>(result: ApiResult<T>): Promise<T> {
  if (result.error) {
    const status = result.response.status;
    throw new ProblemError(status, result.error.code);
  }
  if (result.data === undefined) {
    throw new ProblemError(500);
  }
  return result.data;
}
