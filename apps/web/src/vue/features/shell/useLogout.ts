import { t } from "@fvoci/i18n";
import { ref } from "vue";
import { logout as logoutRequest } from "@/features/notifications/push-logout";
import { ProblemError, problemMessage } from "@/lib/api";
import { redirectTo } from "../../session/navigation";

/** The browser pieces the logout touches; tests pass their own. */
export interface LogoutEnvironment {
  /** `POST /auth/logout`, which also disconnects this browser's Web Push; throws on a transport failure. */
  request(): Promise<{ response: { ok: boolean; status: number } }>;
  redirect(path: string): void;
}

const BROWSER: LogoutEnvironment = { request: logoutRequest, redirect: redirectTo };

/**
 * The workspace shell's logout (features/workspace/workspace-shell.tsx): the
 * request reports and then drops this browser's push subscription
 * (push-logout.ts). A transport failure or a refused logout shows in place
 * and keeps the session; a logout goes to the login page, a React page, with
 * a full load that replaces this entry (the React shell's replace navigation,
 * which also leaves no cached query data behind).
 */
export function useLogout(env: LogoutEnvironment = BROWSER) {
  const error = ref<string | null>(null);

  async function logout(): Promise<void> {
    error.value = null;
    let result: Awaited<ReturnType<LogoutEnvironment["request"]>>;
    try {
      result = await env.request();
    } catch {
      error.value = t("error.network");
      return;
    }
    if (!result.response.ok) {
      error.value = problemMessage(new ProblemError(result.response.status), "error.auth.logout");
      return;
    }
    env.redirect("/login");
  }

  return { error, logout };
}
