<!--
  The organization's policies, together: its name, whether its boards may be
  read by a public link, whether its containers may be copied out of it, and,
  only while it takes its single sign-on alone (`admin.ssoRequired`), whether
  personal API tokens open it then; without that they open it as any session
  does, and there is nothing to switch. Each is sent as it is changed; the
  switches are drawn anew with what the server has after each change, so one
  that was refused flips back. A name being typed stays through the other
  changes, and through a refusal of it.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { OrgAdmin } from '$lib/state/org-admin.svelte.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Switch } from '$lib/ui/switch/index.js';

  let { admin }: { admin: OrgAdmin } = $props();

  const uid = $props.id();
  /** The name the server has: the same after a change of something else, read anew. */
  const named = $derived(admin.organization?.name ?? '');
  /** What the field holds: the name the server has, until it is typed over. */
  let name = $derived(named);
  const renamed = $derived(name.trim() !== named);
</script>

{#snippet policy(
  title: string,
  hint: string,
  checked: boolean,
  testId: string,
  change: (on: boolean) => Promise<boolean>,
)}
  <label class="flex items-start gap-3 rounded-md border px-3 py-2">
    <span class="min-w-0 flex-1">
      <span class="block">{title}</span>
      <span class="block text-xs text-muted-foreground">{hint}</span>
    </span>
    {#key admin.organization}
      <Switch
        {checked}
        disabled={admin.busy}
        data-testid={testId}
        onCheckedChange={(on: boolean) => void change(on)}
      />
    {/key}
  </label>
{/snippet}

<section class="flex flex-col gap-3" aria-labelledby="org-settings" data-testid="org-policy">
  <h2 id="org-settings" class="text-base font-semibold">{m.org_policy()}</h2>
  <form
    class="flex flex-wrap items-end gap-2 rounded-md border px-3 py-2"
    onsubmit={(event) => {
      event.preventDefault();
      if (renamed) void admin.rename(name);
    }}
  >
    <div class="grid min-w-0 flex-1 gap-1">
      <label for="{uid}-name" class="text-sm">{m.org_name()}</label>
      <Input
        id="{uid}-name"
        bind:value={name}
        maxlength={100}
        autocomplete="off"
        disabled={admin.busy}
        class="text-base md:text-sm roomy:h-touch"
        data-testid="org-name"
      />
    </div>
    <Button
      type="submit"
      variant="outline"
      class="roomy:min-h-touch"
      disabled={admin.busy || !renamed}
      data-testid="org-name-save"
    >
      {m.org_name_save()}
    </Button>
  </form>
  {@render policy(
    m.org_public_links(),
    m.org_public_links_hint(),
    admin.organization?.publicLinks ?? true,
    'org-public-links',
    (on) => admin.setPublicLinks(on),
  )}
  {@render policy(
    m.org_allow_copy_out(),
    m.org_allow_copy_out_hint(),
    admin.organization?.allowCopyOut ?? true,
    'org-allow-copy-out',
    (on) => admin.setAllowCopyOut(on),
  )}
  {#if admin.ssoRequired}
    {@render policy(
      m.org_api_tokens_allow(),
      m.org_api_tokens_allow_hint(),
      admin.organization?.allowApiTokens ?? false,
      'org-allow-api-tokens',
      (on) => admin.setAllowApiTokens(on),
    )}
  {/if}
</section>
