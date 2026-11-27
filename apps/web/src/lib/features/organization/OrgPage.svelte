<!--
  The organization screen (`/org/<organization>`), for its administrators
  only: the workspaces (`OrgWorkspaces.svelte`), its single sign-on
  (`OrgSso.svelte`, one line until it is set up), the members and their roles,
  its policies (`OrgPolicy.svelte`: its name, and the switches of the public
  links, of copying out and, while it takes its single sign-on alone, of
  personal API tokens), the members' tokens that reach it
  (`OrgApiTokens.svelte`), and what was done to the organization.
  An administrator manages here; what is in a workspace they read by joining
  it. A refused change is told in a line at the top; the lists show what the
  server has.

  An organization that takes its single sign-on alone, read with a session
  made another way, shows what the app gives it for that (`closed`): the
  way to that sign-in is the app's, not this screen's.
-->
<script lang="ts">
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import type { Snippet } from 'svelte';

  import AuditList from '$lib/components/audit/AuditList.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { OrgAdmin, OrgRefusal } from '$lib/state/org-admin.svelte.ts';
  import type { OidcSettings } from '$lib/state/oidc.ts';
  import type { OrgMember } from '$lib/state/organization.ts';
  import { Button } from '$lib/ui/button/index.js';
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import PersonProfile from '$lib/components/person/PersonProfile.svelte';

  import OrgApiTokens from './OrgApiTokens.svelte';
  import OrgPolicy from './OrgPolicy.svelte';
  import OrgWorkspaces from './OrgWorkspaces.svelte';
  import OrgSso from './OrgSso.svelte';

  interface Props {
    admin: OrgAdmin;
    locale: string;
    /** The reader: stepping down from administering the organization is asked first. */
    meId: string | undefined;
    homeHref: string;
    workspaceHref: (workspaceId: string) => string;
    onSsoSignIn: () => Promise<void>;
    /** What is shown of an organization this session does not open. */
    closed?: Snippet;
  }

  let { admin, locale, meId, homeHref, workspaceHref, closed, onSsoSignIn }: Props = $props();

  const REFUSALS: Record<OrgRefusal, () => string> = {
    name_taken: m.org_refused_name_taken,
    default_workspace: m.org_refused_default_workspace,
    last_org_admin: m.org_refused_last_admin,
    cannot_remove_self: m.org_refused_cannot_remove_self,
    last_instance_admin: m.org_refused_last_instance_admin,
    sole_org_admin: m.org_refused_sole_org_admin,
    alone_in_organization: m.org_refused_alone_in_organization,
    not_found: m.org_refused_not_found,
    failed: m.org_refused_failed,
  };

  let stepDownOpen = $state(false);
  let removing = $state.raw<OrgMember | undefined>(undefined);
  let removeOpen = $state(false);
  let membersHeading: HTMLElement | null = null;

  /** Sends the browser to sign in through the pending settings; it comes back to this page. */
  async function tryPending(): Promise<void> {
    location.assign(await admin.startOidcTrial());
  }

  const named = (member: OrgMember): string =>
    member.displayName ? `${member.displayName} (@${member.username})` : `@${member.username}`;
</script>

<main
  class="mx-auto flex min-h-dvh w-full max-w-3xl flex-col gap-6 px-4 py-6"
  data-testid="org-page"
>
  <header class="flex items-center gap-2">
    <a
      href={homeHref}
      class="inline-flex size-9 items-center justify-center rounded-md hover:bg-accent"
      aria-label={m.org_back()}
      data-testid="org-back"
    >
      <ArrowLeftIcon class="size-5" aria-hidden="true" />
    </a>
    <div class="min-w-0">
      <h1 class="truncate text-xl font-semibold tracking-tight">{m.org_title()}</h1>
      {#if admin.organization?.name}
        <p class="truncate text-sm text-muted-foreground">{admin.organization.name}</p>
      {/if}
    </div>
  </header>

  {#if admin.status === 'loading'}
    <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
  {:else if admin.status === 'missing'}
    <!-- Not theirs to administer: never, or no longer (stepped down, or made a member). -->
    <p class="text-muted-foreground" role="alert" data-testid="org-missing">{m.org_missing()}</p>
  {:else if admin.status === 'sso' && closed}
    {@render closed()}
  {:else if admin.status === 'failed' || admin.status === 'sso'}
    <div>
      <p class="text-muted-foreground" role="alert">{m.app_problem_load()}</p>
      <Button class="mt-4 roomy:min-h-touch" onclick={() => void admin.load()}
        >{m.app_retry()}</Button
      >
    </div>
  {:else}
    {#if admin.refusal}
      <p
        class="rounded-md border border-destructive/50 px-3 py-2 text-sm"
        role="alert"
        data-testid="org-refusal"
      >
        {REFUSALS[admin.refusal]()}
      </p>
    {/if}

    <OrgWorkspaces {admin} {workspaceHref} />

    {#key admin.orgId}
      <OrgSso
        read={() => admin.oidcProvider()}
        save={(settings: OidcSettings) => admin.saveOidcProvider(settings)}
        remove={() => admin.deleteOidcProvider()}
        onSignIn={onSsoSignIn}
        discard={() => admin.discardOidcPending()}
        endFormer={(createdAt: number) => admin.endOidcFormer(createdAt)}
        onTry={tryPending}
      />
    {/key}

    <section class="flex flex-col gap-3" aria-labelledby="org-members">
      <h2
        id="org-members"
        tabindex="-1"
        class="text-base font-semibold outline-none"
        {@attach (node: HTMLElement) => {
          membersHeading = node;
          return () => {
            membersHeading = null;
          };
        }}
      >
        {m.org_members()}
      </h2>
      <ul class="flex flex-col gap-2" data-testid="org-members">
        {#each admin.members as member (member.userId)}
          <li
            class="flex flex-wrap items-center gap-2 rounded-md border px-3 py-2"
            data-username={member.username}
          >
            <span class="flex min-w-0 flex-1">
              <PersonProfile
                userId={member.userId}
                username={member.username}
                displayName={member.displayName}
                name={member.name}
                {locale}
              />
            </span>
            <span class="text-xs text-muted-foreground">
              {member.role === 'admin' ? m.org_role_admin() : m.org_role_member()}
            </span>
            <Button
              size="sm"
              variant="outline"
              disabled={admin.busy}
              aria-label={member.role === 'admin'
                ? member.userId === meId
                  ? m.org_step_down()
                  : m.org_make_member_named({ name: named(member) })
                : m.org_make_admin_named({ name: named(member) })}
              onclick={() => {
                if (member.userId === meId) stepDownOpen = true;
                else
                  void admin.setRole(member.userId, member.role === 'admin' ? 'member' : 'admin');
              }}
            >
              {member.role === 'admin'
                ? member.userId === meId
                  ? m.org_step_down()
                  : m.org_make_member()
                : m.org_make_admin()}
            </Button>
            {#if meId !== undefined && member.userId !== meId}
              <Button
                size="sm"
                variant="outline"
                disabled={admin.busy}
                aria-label={m.org_remove_member_named({ name: named(member) })}
                onclick={() => {
                  removing = member;
                  removeOpen = true;
                }}
              >
                {m.org_remove_member()}
              </Button>
            {/if}
          </li>
        {/each}
      </ul>
    </section>

    <OrgPolicy {admin} />

    {#key admin.orgId}
      <OrgApiTokens
        read={() => admin.apiTokens()}
        revoke={(tokenId: string) => admin.revokeApiToken(tokenId)}
        nameOf={(userId: string) => {
          const member = admin.members.find((one) => one.userId === userId);
          return member ? named(member) : m['centerPanel.unknown-user']();
        }}
      />
    {/key}

    <section class="flex flex-col gap-3" aria-labelledby="org-records">
      <h2 id="org-records" class="text-base font-semibold">{m.org_records()}</h2>
      <AuditList records={admin.records} {locale} testId="org-records" />
    </section>
  {/if}
</main>

<ConfirmDialog
  bind:open={stepDownOpen}
  title={m.org_step_down_title()}
  description={m.org_step_down_info()}
  confirmLabel={m.org_step_down()}
  destructive
  onConfirm={() => {
    if (meId) void admin.setRole(meId, 'member');
  }}
/>

<ConfirmDialog
  bind:open={removeOpen}
  title={m.org_remove_member_title({ name: removing ? named(removing) : '' })}
  description={m.org_remove_member_info()}
  confirmLabel={m.org_remove_member()}
  destructive
  focusAfter={() => membersHeading}
  onConfirm={() => {
    if (removing) void admin.removeMember(removing.userId);
  }}
/>
