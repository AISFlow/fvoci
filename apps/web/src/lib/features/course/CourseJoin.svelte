<!--
  The bar above an open course the reader is no member of: they read it as it
  is, and join it here with the course's minimum role. It is the one place
  that asks: what a member keeps of the course (progress, reviews, attendance,
  questions) is not offered before.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    joining: boolean;
    onJoin: () => void;
  }

  let { joining, onJoin }: Props = $props();

  const uid = $props.id();
</script>

<div
  class="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-2 border-b bg-muted px-3 py-2"
  data-testid="course-join"
>
  <p id="{uid}-hint" class="min-w-0 flex-1 text-sm text-muted-foreground">{m.course_join_hint()}</p>
  <Button
    size="sm"
    class="roomy:min-h-touch"
    aria-describedby="{uid}-hint"
    aria-disabled={joining}
    aria-busy={joining}
    onclick={() => {
      if (!joining) onJoin();
    }}
  >
    {m.course_join()}
  </Button>
</div>
