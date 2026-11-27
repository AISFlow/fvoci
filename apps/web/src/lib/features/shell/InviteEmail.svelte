<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError, type InvitationMail } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Label } from '$lib/ui/label/index.js';

  let {
    mail,
    workspaceId,
    token,
    language,
    hint,
  }: {
    mail: InvitationMail;
    workspaceId: string;
    token: string;
    language: string;
    /** What the addresses get: the same link, or a link of their own each. */
    hint: string;
  } = $props();
  const uid = $props.id();
  const available = $derived(mail.available().catch(() => false));
  let addresses = $state('');
  let working = $state(false);
  let queued = $state(false);
  let problem = $state<string | undefined>(undefined);

  /** What the person is told of a refusal; the server's own words stay out. */
  function refusal(error: unknown): string {
    if (!isBackendError(error)) return m.people_failed();
    switch (error.reason) {
      case 'too_many_recipients':
        return error.limit === undefined
          ? m.mail_too_many_recipients()
          : m.mail_too_many_recipients_limit({ limit: error.limit });
      case 'invitation_mail_limit':
        return m.mail_hourly_limit({
          minutes: Math.max(1, Math.ceil((error.retryAfter ?? 0) / 60)),
        });
      case 'mail_queue_full':
        return m.mail_queue_full();
    }
    return error.refusalAt('/recipients') ? m.mail_addresses_invalid() : m.people_failed();
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working) return;
    working = true;
    problem = undefined;
    queued = false;
    const recipients = addresses
      .split(/[,\n]/)
      .map((address) => address.trim())
      .filter(Boolean);
    try {
      await mail.send(workspaceId, token, recipients, language);
      addresses = '';
      queued = true;
    } catch (error) {
      problem = refusal(error);
    } finally {
      working = false;
    }
  }
</script>

{#await available then enabled}
  {#if enabled}
    <form
      class="mt-2 grid gap-2"
      onsubmit={(event) => void submit(event)}
      data-testid="invite-email-form"
    >
      <Label for="{uid}-recipients">{m.mail_addresses()}</Label>
      <textarea
        id="{uid}-recipients"
        bind:value={addresses}
        rows="3"
        required
        disabled={working}
        aria-describedby="{uid}-hint"
        class="w-full rounded-md border border-input-border bg-background px-3 py-2 text-sm roomy:text-base"
      ></textarea>
      <p id="{uid}-hint" class="text-xs text-muted-foreground">{hint}</p>
      {#if problem}<p class="text-sm text-destructive" role="alert">{problem}</p>{/if}
      {#if queued}<p class="text-sm" role="status">{m.mail_queued()}</p>{/if}
      <Button
        type="submit"
        variant="outline"
        disabled={working}
        class="justify-self-start roomy:min-h-touch">{m.mail_send()}</Button
      >
    </form>
  {/if}
{/await}
