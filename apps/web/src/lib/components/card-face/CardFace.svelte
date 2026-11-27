<!--
  The face of a card: its number, icon and title, the properties the view turned on,
  and what its body holds (that there is one, how its tasks stand). A kanban
  column and a gallery draw the same face, the gallery a larger one with a
  preview of the body: its first picture and the beginning of its text.
  The picture is fetched once it comes near the screen (`loading="lazy"`).

  This is only what a card looks like. What makes it a thing to press, drag
  or select is on the element around it. So is its menu: the caller lays the
  button over the top corner, and `menuRoom` keeps the title clear of it.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import CornerDownRightIcon from '@lucide/svelte/icons/corner-down-right';
  import ListTreeIcon from '@lucide/svelte/icons/list-tree';
  import SquareCheckIcon from '@lucide/svelte/icons/square-check';
  import TextAlignStartIcon from '@lucide/svelte/icons/text-align-start';

  import CardNumber from '$lib/components/card-number/CardNumber.svelte';
  import { PropertyValue, type PropertyUser } from '$lib/components/properties/index.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Regional } from '$lib/logic/regional.ts';
  import type { Card, PropertyTemplate } from '$lib/logic/wire.ts';

  import { faceProperties } from './face.ts';

  interface Props {
    card: Card;
    /** People currently reading this card, supplied by its board. */
    readers?: Snippet;
    /** The definitions of the board's card properties. */
    templates: readonly PropertyTemplate[];
    /** The properties the view shows on its cards, in its order. */
    shownProperties: readonly string[];
    /** The card has a body. */
    hasBody?: boolean;
    /** The tasks in the body: how many, and how many are done. */
    tasksTotal?: number;
    tasksDone?: number;
    /** Direct sub-cards of this card: how many, and how many are completed. */
    subcardsTotal?: number;
    subcardsDone?: number;
    /** Subtle parent card clue when this card is a sub-card. */
    parentHint?: { number?: number; title: string };
    /** `large`: the card of a gallery. */
    size?: 'small' | 'large';
    /** The id of the title, for the element around the face to be named by. */
    titleId?: string;
    locale: string;
    regional: Regional;
    users?: readonly PropertyUser[];
    /** The current moment, for a test. */
    now?: number;
    /** Off the board: under the pointer, or carried by the keyboard. */
    lifted?: boolean;
    selected?: boolean;
    /** How many cards travel with this one, itself included. Shown when more than one. */
    count?: number;
    /** The address of the body's first picture, for a gallery. */
    previewImage?: string;
    /** The beginning of the body's text, for a gallery. */
    previewText?: string;
    /** A menu button lies over the top corner at the end of the title. */
    menuRoom?: boolean;
    /** Classes for the box of the face, such as a height to fill. */
    class?: string;
  }

  let {
    card,
    readers,
    templates,
    shownProperties,
    hasBody = false,
    tasksTotal = 0,
    tasksDone = 0,
    subcardsTotal = 0,
    subcardsDone = 0,
    parentHint,
    size = 'small',
    titleId,
    locale,
    regional,
    users = [],
    now,
    lifted = false,
    selected = false,
    count = 1,
    previewImage,
    previewText,
    menuRoom = false,
    class: className,
  }: Props = $props();

  const shown = $derived(faceProperties(card, templates, shownProperties));
  const title = $derived(card.title || m['KanbanCard.untitled']());
  const large = $derived(size === 'large');
</script>

<div
  data-slot="card-face"
  data-size={size}
  class={[
    'relative rounded-md border bg-card text-card-foreground',
    large ? 'p-4' : 'p-3',
    lifted ? 'shadow-lg ring-2 ring-ring' : 'shadow-xs',
    selected && 'border-primary ring-2 ring-primary',
    className,
  ]}
>
  {#if parentHint}
    <p
      data-slot="card-parent-hint"
      title={m.card_parent_hint({ title: parentHint.title })}
      class={[
        'mb-1 flex min-w-0 items-center gap-1 text-xs text-muted-foreground',
        menuRoom && 'pr-7 roomy:pr-9',
      ]}
    >
      <CornerDownRightIcon class="size-3 shrink-0" aria-hidden="true" />
      <CardNumber number={parentHint.number} />
      <span class="sr-only">{m.card_parent_hint({ title: parentHint.title })}</span>
      <span class="min-w-0 truncate" aria-hidden="true">{parentHint.title}</span>
    </p>
  {/if}

  <!-- Room at the end of the title for the menu button: 32 pixels, 44 where a finger presses. -->
  <p
    id={titleId}
    data-slot="card-title"
    class={[
      'flex min-w-0 items-start gap-1.5 font-medium wrap-anywhere',
      large && 'text-base',
      menuRoom && 'pr-7 roomy:pr-9',
      !card.title && 'text-muted-foreground',
    ]}
  >
    <CardNumber number={card.number} />
    {#if card.fields.icon}
      <span data-slot="card-icon" class="shrink-0" aria-hidden="true">{card.fields.icon}</span>
    {/if}
    <span data-slot="card-title-text" class="min-w-0">{title}</span>
  </p>

  {#if previewImage || previewText}
    <div data-slot="card-preview" class="mt-2 flex flex-col gap-2">
      {#if previewImage}
        <!-- The title names the card; the picture adds nothing a screen reader needs. -->
        <img
          src={previewImage}
          alt=""
          loading="lazy"
          decoding="async"
          class="aspect-video w-full rounded-sm bg-muted object-cover"
        />
      {/if}
      {#if previewText}
        <p class="line-clamp-4 text-sm whitespace-pre-line text-muted-foreground">
          {previewText}
        </p>
      {/if}
    </div>
  {/if}

  {#if shown.length > 0}
    <!-- The kinds bring their own side padding; pulled out so that the values line up with the title. -->
    <div
      data-slot="card-properties"
      class={['-mx-2 flex flex-col text-sm', large ? 'mt-2' : 'mt-1']}
    >
      {#each shown as template (template.id)}
        <div data-property={template.id} title={template.name} class="flex min-w-0 items-center">
          <span class="sr-only">{template.name}:</span>
          <div class={template.type === 'checkbox' ? 'shrink-0' : 'min-w-0 flex-1'}>
            <PropertyValue
              {template}
              {card}
              value={card.fields.properties[template.id]}
              mode="display"
              {users}
              {locale}
              {regional}
              {now}
            />
          </div>
          {#if template.type === 'checkbox'}
            <!-- A box alone says nothing: its property is named next to it. -->
            <span class="min-w-0 truncate text-muted-foreground" aria-hidden="true">
              {template.name}
            </span>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if hasBody || tasksTotal > 0 || subcardsTotal > 0}
    <div
      data-slot="card-badges"
      class={['flex items-center gap-3 text-xs text-muted-foreground', large ? 'mt-3' : 'mt-2']}
    >
      {#if hasBody}
        <span data-slot="card-has-body" title={m['CardBadges.title-description']()}>
          <TextAlignStartIcon class="size-3.5" aria-hidden="true" />
          <span class="sr-only">{m['CardBadges.title-description']()}</span>
        </span>
      {/if}
      {#if tasksTotal > 0}
        <span
          data-slot="card-tasks"
          class="flex items-center gap-1"
          title={m['CardBadges.title-checkboxes']()}
        >
          <SquareCheckIcon class="size-3.5" aria-hidden="true" />
          <span class="sr-only">{m['CardBadges.title-checkboxes']()}</span>
          {tasksDone}/{tasksTotal}
        </span>
      {/if}
      {#if subcardsTotal > 0}
        <span
          data-slot="card-subcards"
          class="flex items-center gap-1"
          title={m.card_subcards_badge({ done: subcardsDone, total: subcardsTotal })}
        >
          <ListTreeIcon class="size-3.5" aria-hidden="true" />
          <span class="sr-only">
            {m.card_subcards_badge({ done: subcardsDone, total: subcardsTotal })}
          </span>
          <span aria-hidden="true">{subcardsDone}/{subcardsTotal}</span>
        </span>
      {/if}
    </div>
  {/if}

  {@render readers?.()}

  {#if count > 1}
    <span
      data-slot="card-count"
      class="absolute -end-2 -top-2 flex h-6 min-w-6 items-center justify-center rounded-full bg-primary px-1.5 text-xs font-semibold text-primary-foreground"
    >
      {count}
    </span>
  {/if}
</div>
