<!--
  Records of an audit log, the newest first: who did what, to what and whom,
  and when. A person is named "display name (@login)"; one whose account is
  gone as such. A board the reader does not see is "a board", not its title.
  An identity a provider's move tied is told under its record.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { AuditPerson, AuditRecord } from '$lib/logic/audit.ts';

  interface Props {
    records: readonly AuditRecord[];
    locale: string;
    /** Omitted in the app: the reader's zone. A fixed zone is useful for examples. */
    timeZone?: string;
    testId?: string;
  }

  let { records, locale, timeZone, testId = 'audit-list' }: Props = $props();

  function named(person: AuditPerson | null): string {
    // An account that was deleted keeps its records, under no name.
    if (!person || person.username === '') return m.people_deleted_user();
    return person.displayName
      ? `${person.displayName} (@${person.username})`
      : `@${person.username}`;
  }

  function said(record: AuditRecord): string {
    const actor =
      record.actor.kind === 'person'
        ? named(record.actor)
        : record.actor.kind === 'server'
          ? m.audit_server()
          : m.audit_server_operator();
    const issuer = record.detail ?? '';
    const subject = named(record.subject);
    const board = record.targetName ?? m.audit_a_board();
    const workspace = record.targetName ?? '';
    switch (record.action) {
      case 'self_added_to_container':
        return m.audit_self_added_to_board({ actor, board });
      case 'added_to_container':
        return m.audit_added_to_board({ actor, subject, board });
      case 'org_admin_joined':
        return m.audit_org_admin_joined({ actor });
      case 'org_admin_left':
        return m.audit_org_admin_left({ actor });
      case 'org_admin_revoked':
        return m.audit_org_admin_revoked({ actor, subject });
      case 'org_admin_granted':
        return m.audit_org_admin_granted({ actor, subject });
      case 'org_admin_removed':
        return m.audit_org_admin_removed({ actor, subject });
      case 'org_member_removed':
        return m.audit_org_member_removed({ actor, subject });
      case 'workspace_created':
        return m.audit_workspace_created({ actor, workspace });
      case 'workspace_renamed':
        return m.audit_workspace_renamed({ actor, workspace });
      case 'workspace_deleted':
        return m.audit_workspace_deleted({ actor, workspace });
      case 'workspace_restored':
        return m.audit_workspace_restored({ actor, workspace });
      case 'public_links_disabled':
        return m.audit_public_links_disabled({ actor });
      case 'oidc_provider_configured':
        return m.audit_oidc_configured({ actor });
      case 'oidc_provider_removed':
        return m.audit_oidc_removed({ actor });
      case 'sso_required':
        return m.audit_sso_required({ actor });
      case 'sso_released':
        return m.audit_sso_released({ actor });
      case 'public_links_enabled':
        return m.audit_public_links_enabled({ actor });
      case 'oidc_provider_pending_saved':
        return m.audit_oidc_pending_saved({ actor });
      case 'oidc_provider_pending_applied':
        return m.audit_oidc_pending_applied({ actor });
      case 'oidc_provider_pending_discarded':
        return m.audit_oidc_pending_discarded({ actor });
      case 'password_reset_link_made':
        return m.audit_password_reset_link_made({ actor, subject });
      case 'password_set_by_operator':
        return m.audit_password_set_by_operator({ actor, subject });
      case 'oidc_former_provider_kept':
        return m.audit_oidc_former_kept({ actor, issuer });
      case 'oidc_former_provider_ended':
        return m.audit_oidc_former_ended({ actor, issuer });
      case 'oidc_former_provider_lapsed':
        return m.audit_oidc_former_lapsed({ issuer });
      case 'oidc_identity_bound':
        return m.audit_oidc_identity_bound({ actor, issuer });
      case 'oidc_identity_carried_over':
        return m.audit_oidc_identity_carried_over({ actor, issuer });
      case 'email_verified_by_operator':
        return m.audit_email_verified_by_operator({ actor, subject });
      case 'container_copied_out':
        return m.audit_container_copied_out({ actor, board });
      case 'copy_out_disabled':
        return m.audit_copy_out_disabled({ actor });
      case 'copy_out_enabled':
        return m.audit_copy_out_enabled({ actor });
      case 'api_tokens_enabled':
        return m.audit_api_tokens_enabled({ actor });
      case 'api_tokens_disabled':
        return m.audit_api_tokens_disabled({ actor });
      case 'api_token_revoked':
        return m.audit_api_token_revoked({ actor, subject, prefix: record.detail ?? '' });
    }
  }

  const when = $derived(
    new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short', timeZone }),
  );
</script>

{#if records.length === 0}
  <p class="text-sm text-muted-foreground" data-testid={testId}>{m.audit_none()}</p>
{:else}
  <ol class="flex flex-col gap-2" data-testid={testId}>
    {#each records as record (record.id)}
      <li class="flex flex-col gap-0.5 rounded-md border px-3 py-2" data-action={record.action}>
        <span class="text-sm">{said(record)}</span>
        {#if record.identity}
          <span class="text-xs break-all" data-testid="audit-identity"
            >{m.audit_identity({ identity: record.identity })}</span
          >
        {/if}
        <time
          class="text-xs text-muted-foreground"
          datetime={new Date(record.createdAt).toISOString()}>{when.format(record.createdAt)}</time
        >
      </li>
    {/each}
  </ol>
{/if}
