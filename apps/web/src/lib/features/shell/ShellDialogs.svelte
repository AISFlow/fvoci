<!--
  Dialogs and overlays owned by `AppShell.svelte`: file transfers, the window of
  new things (`TemplatePicker`), account settings, activity records, and dev readout.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ContainerPlace } from '$lib/logic/urls.ts';
  import type { Container } from '$lib/logic/wire.ts';
  import {
    isOwnSpace,
    type ContainerDraft,
    type NewKind,
    type Workspace,
  } from '$lib/state/index.ts';
  import type { AuditRecord } from '$lib/state/organization.ts';

  import AuditDialog from './AuditDialog.svelte';
  import { devReadout } from './dev.svelte.ts';
  import DevReadout from './DevReadout.svelte';
  import SettingsDialog from './SettingsDialog.svelte';
  import { shownSpaceName } from './space-names.ts';
  import TemplatePicker from './TemplatePicker.svelte';
  import Transfers from './Transfers.svelte';

  interface Props {
    workspace: Workspace;
    picking: boolean;
    settingsOpen: boolean;
    recordsOpen: boolean;
    worksHere: boolean;
    locales?: readonly string[] | undefined;
    language?: string | undefined;
    autoLocale?: string | undefined;
    onChooseLanguage?: ((setting: string) => void) | undefined;
    onSignOut?: (() => void) | undefined;
    onChangePassword?: ((oldPassword: string, newPassword: string) => Promise<void>) | undefined;
    accountSettings?: Snippet | undefined;
    containerHref: (container: ContainerPlace) => string;
    onOpenContainer: (container: ContainerPlace) => void;
    onCreate: (kind: NewKind, draft: ContainerDraft) => void;
    onFromTemplate: (template: Container, draft: ContainerDraft) => void;
    readRecords?: ((workspaceId: string) => Promise<AuditRecord[]>) | undefined;
  }

  let {
    workspace,
    picking = $bindable(),
    settingsOpen = $bindable(),
    recordsOpen = $bindable(),
    worksHere,
    locales,
    language,
    autoLocale,
    onChooseLanguage,
    onSignOut,
    onChangePassword,
    accountSettings,
    containerHref,
    onOpenContainer,
    onCreate,
    onFromTemplate,
    readRecords,
  }: Props = $props();
</script>

<Transfers {workspace} />

<TemplatePicker
  bind:open={picking}
  {workspace}
  alone={isOwnSpace(workspace.info)}
  workspaceName={shownSpaceName(workspace, m.shell_workspace_unnamed)}
  {onCreate}
  {onFromTemplate}
  onEditTemplate={onOpenContainer}
/>

<SettingsDialog
  bind:open={settingsOpen}
  {workspace}
  {locales}
  {language}
  {autoLocale}
  {onChooseLanguage}
  {onSignOut}
  {onChangePassword}
  account={accountSettings}
  {containerHref}
  onOpenContainer={(container: ContainerPlace) => {
    settingsOpen = false;
    onOpenContainer(container);
  }}
  onOpenRecords={readRecords && worksHere
    ? () => {
        settingsOpen = false;
        recordsOpen = true;
      }
    : undefined}
/>

{#if readRecords}
  <AuditDialog
    bind:open={recordsOpen}
    workspaceId={workspace.info?.id}
    locale={workspace.locale}
    {readRecords}
  />
{/if}

{#if devReadout.shown}
  <DevReadout />
{/if}
