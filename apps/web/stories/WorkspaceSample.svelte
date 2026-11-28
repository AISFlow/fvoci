<script lang="ts">
  import WorkspaceSwitcher from '$lib/features/shell/WorkspaceSwitcher.svelte';
  import { BackendError, type WorkspaceInfo } from '$lib/state/index.ts';

  let {
    count = 4,
    long = false,
    own = false,
    inOwn = false,
    locked = false,
    starts,
  }: {
    count?: number;
    long?: boolean;
    /** The reader's own space is listed, last as the server lists it. */
    own?: boolean;
    /** The own space is the one shown. */
    inOwn?: boolean;
    /** The own space takes the password alone, and this session was made through a provider. */
    locked?: boolean;
    /** Organizations can be started: made, or refused as the server refuses one past its limit. */
    starts?: 'made' | 'limit';
  } = $props();

  const listed = $derived<WorkspaceInfo[]>([
    ...Array.from({ length: count }, (_, index) => ({
      id: `example-workspace-${index}`,
      // The first two share an organization, grouped under its name; the others are alone in theirs.
      orgId: index < 2 ? 'example-org-a' : `example-org-${index}`,
      kind: 'shared' as const,
      title:
        index === 0 && long
          ? 'Example workspace · 이름이 긴 작업 공간 '.repeat(8)
          : `Example workspace ${index} · 예시 작업 공간`,
    })),
    ...(own
      ? [{ id: 'own-space', kind: 'personal' as const, title: '', passwordRequired: locked }]
      : []),
  ]);
  let started = $state<WorkspaceInfo[]>([]);
  const workspaces = $derived([...started, ...listed]);
  let current = $derived(inOwn ? 'own-space' : listed[0]?.id);
  const orgName = (orgId: string | undefined): string =>
    orgId === 'example-org-a'
      ? 'Example organization · 예시 조직'
      : orgId
        ? `Org ${orgId.slice(12)}`
        : '';

  async function createOrganization(name: string): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 50));
    if (starts === 'limit') {
      throw new BackendError(400, { reason: 'too_many_organizations', limit: 3 });
    }
    const made = { id: `started-${started.length}`, kind: 'shared' as const, title: name };
    started = [...started, made];
    current = made.id;
  }
</script>

<div class="grid max-w-sm gap-4">
  <div class="flex min-w-0 bg-sidebar p-2">
    <WorkspaceSwitcher
      {workspaces}
      {current}
      {orgName}
      onSwitch={(id: string) => {
        current = id;
      }}
      onCreateOrganization={starts ? createOrganization : undefined}
    />
  </div>
  <output data-testid="workspace-choice" class="text-xs text-muted-foreground">
    {current ?? ''}
  </output>
</div>
