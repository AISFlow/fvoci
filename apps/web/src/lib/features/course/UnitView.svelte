<!--
  A unit of a course: its title, who else is on it now (`viewers`), and its
  body (`body`). Mirrors `PageView.svelte` without an icon picker (units have
  no icon field on the server).
-->
<script lang="ts">
  import BellIcon from '@lucide/svelte/icons/bell';
  import BellOffIcon from '@lucide/svelte/icons/bell-off';
  import { onDestroy, untrack, type Snippet } from 'svelte';

  import { autosize } from '$lib/components/autosize.ts';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isComposing } from '$lib/keys.ts';
  import type { CourseStore, CourseUnit, UnitStore } from '$lib/state/index.ts';

  import UnitProgressControls from './UnitProgressControls.svelte';
  import UnitScheduleControls from './UnitScheduleControls.svelte';
  import type { UnitOpenings } from './unit-openings.svelte.ts';

  interface Props {
    course: CourseStore;
    store: UnitStore;
    unit: CourseUnit;
    /** What the unit's menu opens below its head. */
    openings: UnitOpenings;
    /** Start with the caret in the title: for a unit that was just made. */
    autofocus?: boolean;
    /** Live viewers badge, supplied by the app shell. */
    viewers?: Snippet | undefined;
    /** Extra head actions (e.g. versions menu), supplied by the app shell. */
    actions?: Snippet<[UnitStore]> | undefined;
    body: Snippet<[UnitStore]>;
  }

  let {
    course,
    store,
    unit,
    openings,
    autofocus = false,
    viewers,
    actions,
    body,
  }: Props = $props();

  const PAUSE = 800;

  const editable = $derived(course.canEdit);

  let field = $state<HTMLTextAreaElement>();
  let draft = $state(untrack(() => unit.title));
  let editing = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let unsaved: string | undefined;

  $effect(() => {
    const stored = unit.title;
    if (!untrack(() => editing)) draft = stored;
  });

  function save(): void {
    clearTimeout(timer);
    timer = undefined;
    if (unsaved === undefined) return;
    const title = unsaved.trim();
    unsaved = undefined;
    if (title !== unit.title) void course.changeUnitTitle(store.unitId, title);
  }

  function oninput(event: Event & { currentTarget: HTMLTextAreaElement }): void {
    draft = event.currentTarget.value.replace(/\s*[\r\n]+\s*/g, ' ');
    unsaved = draft;
    clearTimeout(timer);
    timer = setTimeout(save, PAUSE);
  }

  function onkeydown(event: KeyboardEvent): void {
    if (isComposing(event)) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      field?.blur();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      clearTimeout(timer);
      unsaved = undefined;
      draft = unit.title;
      field?.blur();
    }
  }

  function onblur(): void {
    editing = false;
    const typed = unsaved !== undefined;
    save();
    draft = typed ? draft.trim() : unit.title;
  }

  $effect(() => {
    const target = field;
    if (autofocus && editable && target)
      untrack(() => {
        target.focus();
      });
  });

  onDestroy(save);

  const TITLE = 'w-full min-w-0 text-2xl font-semibold tracking-tight @columns/pane:text-3xl';
</script>

<article
  class="mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-6 @columns/pane:px-8"
  aria-label={unit.title || m.course_unit_untitled()}
  data-testid="unit-view"
  data-unit={unit.id}
>
  <header class="flex flex-col items-start gap-2" data-testid="unit-heading">
    <div class="flex w-full items-start gap-2">
      <div class="min-w-0 flex-1">
        {#if editable}
          <textarea
            bind:this={field}
            bind:value={draft}
            rows="1"
            data-testid="unit-title"
            aria-label={m.course_unit_title()}
            placeholder={m.course_unit_untitled()}
            autocomplete="off"
            enterkeyhint="done"
            class={[
              TITLE,
              'field-sizing-content resize-none overflow-hidden rounded-md bg-transparent placeholder:text-muted-foreground roomy:min-h-touch',
            ]}
            onfocus={() => {
              editing = true;
            }}
            {oninput}
            {onkeydown}
            {onblur}
            {@attach autosize(() => draft)}></textarea>
        {:else}
          <h1
            class={[TITLE, unit.title === '' && 'text-muted-foreground']}
            data-testid="unit-title"
          >
            {unit.title || m.course_unit_untitled()}
          </h1>
        {/if}
      </div>
      {#if viewers !== undefined || actions !== undefined || (store.canWatch && store.watching !== undefined)}
        <div class="flex shrink-0 items-center gap-1 pt-1">
          {@render viewers?.()}
          {#if store.canWatch && store.watching !== undefined}
            <IconButton
              label={store.watching ? m.course_unit_watching() : m.course_unit_watch()}
              aria-pressed={store.watching}
              disabled={store.watchingBusy}
              data-testid="unit-watch-toggle"
              class={store.watching ? 'text-primary' : 'text-muted-foreground'}
              onclick={() => void store.setWatching(!store.watching)}
            >
              {#if store.watching}
                <BellIcon aria-hidden="true" />
              {:else}
                <BellOffIcon aria-hidden="true" />
              {/if}
            </IconButton>
          {/if}
          {@render actions?.(store)}
        </div>
      {/if}
    </div>
    <UnitProgressControls {course} unitId={unit.id} />
    <UnitScheduleControls {course} {unit} {openings} />
  </header>
  {@render body(store)}
</article>
