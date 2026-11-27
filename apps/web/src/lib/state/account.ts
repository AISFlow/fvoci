/** The reader's own account: what the workspace changes of it beside the settings. */
import type { NameChange } from '../logic/person-names.ts';

import { BackendError, isBackendError, isOwnSpace, sessionOver, type User } from './backend.ts';
import type { Workspace } from './workspace.svelte.ts';

/**
 * Changes what the patch names of the reader's name. Every list of people
 * the workspace holds then has the new name. A refusal is thrown, for the
 * field to say what is wrong (`errors` at the member, `/firstName`).
 */
export async function changeName(workspace: Workspace, patch: NameChange): Promise<void> {
  takeUser(workspace, await workspace.backend.patchMe(patch));
}

/**
 * Gives `password`, which the account was just given, in the session where
 * the reader's own space asks for it (`Backend.confirmPassword`): a session
 * made through a provider, as the server lists the workspaces to it. The
 * reader is not asked for what they typed a moment ago. Whether the own
 * space is open to the session afterwards; a refusal (too many tries, a
 * password changed again meanwhile) or a failed reading leaves it as it is.
 */
async function showPassword(workspace: Workspace, password: string): Promise<boolean> {
  try {
    let listed = await workspace.backend.workspaces();
    if (listed.some((one) => one.passwordRequired)) {
      await workspace.backend.confirmPassword(password);
      listed = await workspace.backend.workspaces();
    }
    workspace.workspaces = listed;
    workspace.pruneWorkspaces();
    return !listed.some((one) => one.passwordRequired);
  } catch (error) {
    if (sessionOver(error)) throw error;
    return false;
  }
}

/**
 * Gives the reader's account its first password, `password`, through `set`
 * (`Account.setPassword`, which throws what the server refuses it with). Set,
 * the account has one, and its own space takes only a session that has shown
 * it: this one shows the password it was just given, and the space is entered
 * again where it is shown, the session's as it is now.
 *
 * The server stops telling the session of the space before it answers, and
 * refuses what the page reads of it until the password is shown. For that
 * moment the space is not closed (`Workspace.showing`): the reader is not
 * shown a locked space on the way to an open one. Where the session cannot
 * show the password after all, the page goes on as the server answers it from
 * then on (`Workspace.report`): the own space asks for the password.
 */
export async function setFirstPassword(
  workspace: Workspace,
  set: (password: string) => Promise<void>,
  password: string,
): Promise<void> {
  let shown = false;
  let setDone = false;
  let ended: BackendError | undefined;
  workspace.showing = true;
  try {
    await set(password);
    setDone = true;
    const me = workspace.me;
    if (me) workspace.me = { ...me, hasPassword: true };
    shown = await showPassword(workspace, password);
  } catch (error) {
    if (isBackendError(error) && sessionOver(error)) ended = error;
    else throw error;
  } finally {
    workspace.showing = false;
  }
  if (ended !== undefined) {
    workspace.report('load', ended);
    if (!setDone) throw ended;
    return;
  }
  if (!shown) {
    const detail = 'a password was set';
    workspace.report(
      'load',
      new BackendError(401, { reason: 'password_sign_in_required', detail }),
    );
    return;
  }
  const own = workspace.info ?? workspace.closed;
  if (workspace.asked !== undefined && own && isOwnSpace(own)) {
    await workspace.enter(workspace.asked, true);
  }
}

/**
 * The reader changed the account's password in this session: where its own
 * space asked the session for the password, the session shows the new one,
 * and the space, if it is the one shown, opens.
 */
export async function passwordChanged(workspace: Workspace, password: string): Promise<void> {
  let shown: boolean;
  try {
    shown = await showPassword(workspace, password);
  } catch (error) {
    if (sessionOver(error)) {
      workspace.report('load', error);
      return;
    }
    throw error;
  }
  if (!shown) return;
  if (workspace.entered === 'password' && workspace.asked !== undefined) {
    await workspace.enter(workspace.asked, true);
  }
}

/**
 * The reader gives the account's password in the session they have
 * (`Backend.confirmPassword`), and their own space opens to it where it is:
 * no other session is made, and nothing else of the page starts again. The
 * workspaces are read as the server lists them to the session now, and the
 * one shown, if it is still the space that asked, is entered. A refusal (a
 * wrong password, too many tries) rejects, for the form to say.
 */
export async function confirmPassword(workspace: Workspace, password: string): Promise<void> {
  await workspace.backend.confirmPassword(password);
  workspace.workspaces = await workspace.backend.workspaces();
  workspace.pruneWorkspaces();
  // Not a workspace the reader went to while this was under way.
  if (workspace.entered === 'password' && workspace.asked !== undefined) {
    await workspace.enter(workspace.asked, true);
  }
}

/**
 * Reads the account's address anew, and the one it waits to change to: after
 * the reader asked for a change or took one back, and where the settings show
 * them, since the link is mostly opened in another browser. A failed reading
 * leaves them as they were.
 */
export async function rereadEmail(workspace: Workspace): Promise<void> {
  const read = await workspace.backend.me().catch(() => undefined);
  const me = workspace.me;
  if (read && me?.id === read.id) {
    workspace.me = { ...me, email: read.email, pendingEmail: read.pendingEmail };
  }
}

/** Puts the user as they are now wherever the workspace holds them. */
export function takeUser(workspace: Workspace, changed: User): void {
  const replace = (users: User[]): User[] =>
    users.map((user) => (user.id === changed.id ? changed : user));
  // Kept from the reader's own reading where another reading leaves them out.
  const me = workspace.me;
  if (me?.id === changed.id) {
    workspace.me = {
      ...changed,
      isAdmin: changed.isAdmin ?? me.isAdmin,
      hasPassword: changed.hasPassword ?? me.hasPassword,
      email: changed.email || me.email,
      pendingEmail: changed.pendingEmail ?? me.pendingEmail,
    };
  }
  workspace.users = workspace.user(changed.id)
    ? replace(workspace.users)
    : [...workspace.users, changed];
  workspace.workspaceUsers = replace(workspace.workspaceUsers);
}
