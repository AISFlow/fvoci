<!--
  Where the notice to set a password stands (`features/account/PasswordNotice`):
  at the foot of the pane, in the reader's own space, for an account that has
  no password, once something is made there, until it is closed for good. That
  it was closed is kept with the account (the client state), so that no other
  browser shows it again. Set, the account has a password, and the own space
  takes only a session that has shown it, which this one then does with the
  password it was given (`setFirstPassword`): the space stays open, and the
  notice stays to say that it is set and what the user name is, until closed.

  A session too old to vouch is signed in again through the provider of the
  first of the reader's organizations that has one, and comes back here.

  An account that has a password is told one thing in the same place, until
  that is closed for good too (`features/account/EmailNotice`): where the mail
  that resets it goes, or that there is no address for it to go to. Not in
  the page the password was just set in, and not where the server sends no
  mail.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import EmailNotice from '$lib/features/account/EmailNotice.svelte';
  import Notice from '$lib/features/account/PasswordNotice.svelte';
  import { isOwnSpace, setFirstPassword, type Account, type Workspace } from '$lib/state/index.ts';

  import { signInOrganizations } from './sign-in-organizations.ts';

  interface Props {
    workspace: Workspace;
    account: Account;
    /** Sends the browser through the organization's provider, back to this page. */
    onProvider: (orgId: string) => Promise<void>;
  }

  let { workspace, account, onProvider }: Props = $props();

  /** The entry of the client state that says the notice was closed for good. */
  const DISMISSED = 'passwordNoticeDismissed';
  /** The same of the line about the account's address. */
  const EMAIL_DISMISSED = 'emailNoticeDismissed';

  /**
   * The password is being set in this page, or was: the notice that asked stays
   * through what follows (the space is entered again) to say that it is set.
   */
  let setHere = $state(false);
  const own = $derived(isOwnSpace(workspace.info));
  const asks = $derived(setHere || (own && workspace.me?.hasPassword === false));
  /** The account has a password: its address is where a reset of it is mailed. */
  const tells = $derived(!setHere && own && workspace.me?.hasPassword === true);
  /** The entry of whichever could be shown. */
  const entry = $derived(asks ? DISMISSED : tells ? EMAIL_DISMISSED : undefined);
  /**
   * Whether it is not to be shown, read once it could be: closed for good, or, of the line
   * about the address, on a server that mails nothing. A failed reading shows the notice
   * to set a password, and not the line.
   */
  const dismissed = $derived.by(() => {
    const kept = entry;
    if (kept === undefined) return undefined;
    // Read once for the entry: whatever the reading itself looks at on its way (the
    // workspace entered) does not read it again, which would make the notice anew.
    return untrack(() => {
      const gone = workspace.backend.screenState().then((state) => state[kept] === '1');
      if (kept === DISMISSED) return gone.catch(() => false);
      const mails = account.loginOptions(undefined).then((options) => options.mailEnabled === true);
      return Promise.all([gone, mails])
        .then(([isGone, doesMail]) => isGone || !doesMail)
        .catch(() => true);
    });
  });
  const made = $derived(setHere || workspace.containers.some((container) => !container.isTemplate));
  /** Closed in this page. */
  let closed = $state(false);

  function dismiss(kept: string): void {
    closed = true;
    void workspace.backend.patchScreenState({ [kept]: '1' }).catch(() => undefined);
  }

  /**
   * Set, the account has a password and its own space takes only a session that
   * has shown it: this one shows it. The line about the address is for another
   * visit: the reader was just asked one thing.
   */
  async function setPassword(password: string): Promise<void> {
    setHere = true;
    try {
      await setFirstPassword(workspace, (given) => account.setPassword(given), password);
    } catch (error) {
      setHere = false;
      throw error;
    }
  }

  async function signInAgain(): Promise<void> {
    const [first] = await signInOrganizations(workspace.workspaces, (orgId) =>
      account.loginOptions(orgId),
    );
    if (!first) throw new Error('no provider of the reader');
    await onProvider(first.orgId);
  }
</script>

{#if dismissed && made && !closed}
  {#await dismissed then gone}
    {#if !gone}
      <!-- Where the header of a view folds, a board has its round "New" at the foot: above it. -->
      <div
        class="pointer-events-none absolute inset-x-0 bottom-0 z-20 flex justify-center p-3 pb-[calc(5.5rem+var(--safe-bottom))] @view-row/pane:pb-[calc(0.75rem+var(--safe-bottom))]"
      >
        <div class="pointer-events-auto w-full max-w-md">
          {#if asks}
            <Notice
              username={workspace.me?.username ?? ''}
              onSet={(password: string) => setPassword(password)}
              onSignInAgain={signInAgain}
              onDismiss={() => {
                dismiss(DISMISSED);
              }}
              onClose={() => {
                closed = true;
              }}
            />
          {:else}
            <EmailNotice
              hasAddress={workspace.me?.email !== ''}
              onDismiss={() => {
                dismiss(EMAIL_DISMISSED);
              }}
            />
          {/if}
        </div>
      </div>
    {/if}
  {/await}
{/if}
