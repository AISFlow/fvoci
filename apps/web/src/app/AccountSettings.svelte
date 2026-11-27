<!--
  The part of the settings that is the reader's account, where the features
  of the account meet: its address, the sign-ins connected to it, the
  personal API tokens it gives agents, leaving an organization, and deleting
  it. A session that is over is told as everywhere (`signedIn`).

  Left, an organization's workspaces are not among the reader's any more: the
  page goes from one of them to the workspace it would start in
  (`Workspace.firstChoice`), and the app forgets it administered it
  (`onLeft`).
-->
<script lang="ts">
  import { onMount } from 'svelte';

  import AccountDeletion from '$lib/features/account/AccountDeletion.svelte';
  import ApiTokensSection from '$lib/features/account/ApiTokensSection.svelte';
  import { tokenWorkspaces } from '$lib/features/account/api-tokens.ts';
  import LinkedSignIns from '$lib/features/account/LinkedSignIns.svelte';
  import OrganizationLeaving from '$lib/features/account/OrganizationLeaving.svelte';
  import { leavableOrganizations } from '$lib/features/account/organization-leaving.ts';
  import {
    isBackendError,
    isOwnSpace,
    leaveOrganization,
    type Account,
    type LinkedIdentity,
    type NewApiToken,
    type UserOrganization,
    type Workspace,
  } from '$lib/state/index.ts';

  import EmailChangeHost from './EmailChangeHost.svelte';
  import type { Router } from './router.svelte.ts';
  import { signInOrganizations } from './sign-in-organizations.ts';

  interface Props {
    workspace: Workspace;
    account: Account;
    router: Router;
    /** Does it; a session that is over is told, and passed on. */
    signedIn: <T>(doing: () => Promise<T>) => Promise<T>;
    /** Connects a sign-in through the organization's provider, which the browser goes to. */
    onLink: (orgId: string, password: string) => Promise<void>;
    onUnlink: (identity: LinkedIdentity, password: string) => Promise<void>;
    /** A sign-in was connected just now. */
    justLinked: boolean;
    /** The connected sign-ins were asked for by name. */
    linkedAsked: boolean;
    /** Asks for the account to be deleted. */
    onDelete: (username: string, password: string) => Promise<void>;
    /** The reader left the organization. */
    onLeft: (orgId: string) => void;
  }

  let {
    workspace,
    account,
    router,
    signedIn,
    onLink,
    onUnlink,
    justLinked,
    linkedAsked,
    onDelete,
    onLeft,
  }: Props = $props();

  let organizations = $state.raw<UserOrganization[]>([]);
  let unread = $state(false);
  let loadToken = 0;

  async function loadOrganizations(): Promise<void> {
    const token = ++loadToken;
    unread = false;
    try {
      const listed = await signedIn(() => workspace.backend.organizations());
      if (token === loadToken) {
        organizations = listed;
        unread = false;
      }
    } catch {
      if (token === loadToken) unread = true;
    }
  }

  onMount(() => {
    void loadOrganizations();
    return () => {
      loadToken += 1;
    };
  });

  const leavable = $derived(leavableOrganizations(organizations));

  /** Forgets the organization left, and goes from its workspace if that was `shown`. */
  function forget(orgId: string, shown: boolean): void {
    organizations = organizations.filter((organization) => organization.id !== orgId);
    onLeft(orgId);
    if (shown) router.go({ workspaceId: workspace.firstChoice() });
    void loadOrganizations();
  }

  async function leave(orgId: string): Promise<void> {
    // Asked before: the workspace shown may hear of the leaving first.
    const shown = workspace.info?.orgId === orgId;
    try {
      await signedIn(() =>
        leaveOrganization(workspace, (left: string) => account.leaveOrganization(left), orgId),
      );
    } catch (error) {
      // Out of it already, left in another tab: as if left now.
      if (isBackendError(error) && error.status === 404) forget(orgId, shown);
      throw error;
    }
    forget(orgId, shown);
  }
</script>

<EmailChangeHost {workspace} {account} />
<LinkedSignIns
  read={() => signedIn(() => account.identities())}
  organizations={() =>
    signInOrganizations(workspace.workspaces, (orgId: string) => account.loginOptions(orgId))}
  {onLink}
  {onUnlink}
  {justLinked}
  reveal={linkedAsked}
/>
<ApiTokensSection
  read={() => signedIn(() => account.apiTokens.list())}
  issue={(token: NewApiToken) => signedIn(() => account.apiTokens.issue(token))}
  revoke={(tokenId: string) => signedIn(() => account.apiTokens.revoke(tokenId))}
  workspaces={tokenWorkspaces(workspace.workspaces)}
  currentWorkspaceId={isOwnSpace(workspace.info) ? undefined : workspace.info?.id}
  hasOwnSpace={workspace.workspaces.some(isOwnSpace)}
  hasPassword={workspace.me?.hasPassword !== false}
  origin={location.origin}
/>
<OrganizationLeaving
  organizations={leavable}
  hasPassword={workspace.me?.hasPassword !== false}
  {unread}
  onRetry={() => void loadOrganizations()}
  onLeave={leave}
/>
{#if workspace.me}
  <AccountDeletion
    username={workspace.me.username}
    read={() => signedIn(() => account.deletion())}
    {onDelete}
  />
{/if}
