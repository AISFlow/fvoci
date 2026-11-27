<!--
  What the reader is told of: a switch for each kind of notification, saved
  at once as every setting is (`workspace.changeSettings`). What is off is
  not made at all: turned on again, nothing of the time between comes.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    NOTIFY_SWITCHES,
    type NotificationKind,
    type NotifySettings,
  } from '$lib/logic/notify-settings.ts';
  import type { Workspace } from '$lib/state/index.ts';
  import { Switch } from '$lib/ui/switch/index.js';

  let { workspace }: { workspace: Workspace } = $props();

  const uid = $props.id();

  const WORDS: Record<NotificationKind, { label: () => string; hint: () => string }> = {
    mention: {
      label: m.shell_settings_notify_mention,
      hint: m.shell_settings_notify_mention_hint,
    },
    assignment: {
      label: m.shell_settings_notify_assignment,
      hint: m.shell_settings_notify_assignment_hint,
    },
    comment: {
      label: m.shell_settings_notify_comment,
      hint: m.shell_settings_notify_comment_hint,
    },
    due_soon: {
      label: m.shell_settings_notify_due,
      hint: m.shell_settings_notify_due_hint,
    },
  };

  function change(setting: keyof NotifySettings, on: boolean): void {
    void workspace.changeSettings({ [setting]: on });
  }
</script>

<section class="grid gap-3 border-t pt-4" aria-labelledby="{uid}-notify">
  <div class="grid gap-0.5">
    <h3
      id="{uid}-notify"
      class="text-xs font-semibold tracking-wide text-muted-foreground uppercase"
    >
      {m.shell_settings_notify()}
    </h3>
    <p class="text-xs text-muted-foreground">{m.shell_settings_notify_hint()}</p>
  </div>
  {#each NOTIFY_SWITCHES as { kind, setting } (setting)}
    <label class="flex min-h-8 items-center gap-3 roomy:min-h-touch" data-setting={setting}>
      <span class="grid min-w-0 flex-1 gap-0.5">
        <span>{WORDS[kind].label()}</span>
        <span class="text-xs text-muted-foreground">{WORDS[kind].hint()}</span>
      </span>
      <Switch
        checked={workspace.settings[setting]}
        onCheckedChange={(on: boolean) => {
          change(setting, on);
        }}
      />
    </label>
  {/each}
</section>
