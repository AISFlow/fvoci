<!--
  A person without a picture: one character of their name on a colour that is
  always the same for that person.

  Next to a name that is already written out the picture adds nothing for a
  screen reader: pass `decorative`.
-->
<script lang="ts">
  import type { ClassValue } from 'svelte/elements';

  import { avatarColor, avatarInitial } from './avatar.ts';

  interface Props {
    /** The name shown for the person. */
    name: string;
    /** What the colour is picked by: the person's identifier. Defaults to the name. */
    colorKey?: string;
    /** The reader's language, for uppercasing and for where a character ends. */
    locale: string;
    decorative?: boolean;
    class?: ClassValue;
  }

  let { name, colorKey, locale, decorative = false, class: className = 'size-6' }: Props = $props();
</script>

<span
  role={decorative ? undefined : 'img'}
  aria-label={decorative ? undefined : name}
  aria-hidden={decorative ? 'true' : undefined}
  data-avatar
  class={[
    'inline-flex shrink-0 items-center justify-center rounded-full text-xs font-medium text-foreground select-none',
    className,
  ]}
  style:background-color="var(--option-{avatarColor(colorKey ?? name)})"
>
  {avatarInitial(name, locale)}
</span>
