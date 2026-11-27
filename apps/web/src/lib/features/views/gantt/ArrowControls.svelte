<!--
  Interactive hit targets, lag/kind badges, and accessible controls for Gantt
  dependency arrows.

  Sits after the row list so pointer clicks reach arrow shafts outside bars and
  grips (`hitPath` is trimmed clear of both bars and grips), and always clips
  its left edge to `scroller.scrollLeft` (including at 0px) so arrows or badges
  near the axis start never paint over or intercept clicks on the sticky title
  column.

  All editable arrow triggers share one roving Tab stop (walking with Arrow
  keys, Home, and End), matching `BarFocus`. Invisible default `FS 0` triggers
  remain `pointer-events-none` until hovered/focused/selected so they never
  block bars beneath them. In read-only mode, static badges are
  `pointer-events-none` and no edit controls are rendered.
-->
<script lang="ts">
  import type { ArrowLine } from './chart.svelte.ts';

  interface Props {
    arrows: readonly ArrowLine[];
    width: number;
    rows: number;
    editable: boolean;
    scroller: HTMLElement | undefined;
    selectedKey?: string;
    hoveredKey?: string;
    focusedKey?: string;
    onhover?: (key: string | undefined) => void;
    onfocuskey?: (key: string | undefined) => void;
    onselect?: (arrow: ArrowLine, anchor: HTMLElement) => void;
    onfocused?: (button: HTMLElement) => void;
  }

  let {
    arrows,
    width,
    rows,
    editable,
    scroller,
    selectedKey,
    hoveredKey,
    focusedKey,
    onhover,
    onfocuskey,
    onselect,
    onfocused,
  }: Props = $props();

  let overlay: HTMLDivElement | undefined;
  let lastFocusedKey = $state<string | undefined>(undefined);

  const tabStopKey = $derived.by(() => {
    if (lastFocusedKey !== undefined && arrows.some((arrow) => arrow.key === lastFocusedKey)) {
      return lastFocusedKey;
    }
    return arrows[0]?.key;
  });

  function syncClip(node: HTMLDivElement): (() => void) | undefined {
    overlay = node;
    const box = scroller;
    if (!box) {
      node.style.clipPath = 'inset(0px 0px 0px 0px)';
      return () => {
        overlay = undefined;
      };
    }
    const update = (): void => {
      node.style.clipPath = `inset(0px 0px 0px ${box.scrollLeft}px)`;
    };
    update();
    box.addEventListener('scroll', update, { passive: true });
    return () => {
      box.removeEventListener('scroll', update);
      node.style.clipPath = '';
      overlay = undefined;
    };
  }

  function triggerFor(arrow: ArrowLine): HTMLElement | null {
    return (
      overlay?.querySelector<HTMLElement>(
        `[data-arrow-trigger][data-from="${CSS.escape(arrow.from)}"][data-to="${CSS.escape(arrow.to)}"]`,
      ) ?? null
    );
  }

  function triggerKeys(event: KeyboardEvent, index: number): void {
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const last = arrows.length - 1;
    const nextIndex = {
      ArrowDown: Math.min(last, index + 1),
      ArrowRight: Math.min(last, index + 1),
      ArrowUp: Math.max(0, index - 1),
      ArrowLeft: Math.max(0, index - 1),
      Home: 0,
      End: last,
    }[event.key];
    if (nextIndex === undefined) return;
    event.preventDefault();
    const target = arrows[nextIndex];
    if (!target) return;
    lastFocusedKey = target.key;
    triggerFor(target)?.focus();
  }
</script>

{#if arrows.length > 0 && rows > 0}
  <div
    data-testid="gantt-arrow-controls"
    class="pointer-events-none absolute top-0 left-(--gantt-title) z-5 w-(--gantt-width)"
    style:height="calc(var(--gantt-row) * {rows})"
    {@attach syncClip}
  >
    {#if editable}
      <svg
        aria-hidden="true"
        class="pointer-events-none absolute inset-0 size-full"
        viewBox="0 0 {width} {rows}"
        preserveAspectRatio="none"
      >
        {#each arrows as arrow (arrow.key)}
          <!-- svelte-ignore a11y_no_static_element_interactions (a wider target for the pointer; each arrow's button below is the one for assistive technology) -->
          <!-- svelte-ignore a11y_click_events_have_key_events (the keys are each arrow's button's, below) -->
          <path
            data-arrow-hit
            data-from={arrow.from}
            data-to={arrow.to}
            d={arrow.hitPath}
            fill="none"
            stroke="transparent"
            stroke-width="14"
            stroke-linejoin="round"
            vector-effect="non-scaling-stroke"
            class="pointer-events-auto cursor-pointer"
            onmouseenter={() => {
              onhover?.(arrow.key);
            }}
            onmouseleave={() => {
              if (hoveredKey === arrow.key) onhover?.(undefined);
            }}
            onclick={() => {
              const trigger = triggerFor(arrow);
              if (trigger) onselect?.(arrow, trigger);
            }}
          />
        {/each}
      </svg>
    {/if}

    {#each arrows as arrow, index (arrow.key)}
      {@const active =
        hoveredKey === arrow.key || focusedKey === arrow.key || selectedKey === arrow.key}
      {@const shown = arrow.badge !== '' || active}
      {#if editable}
        <button
          type="button"
          data-arrow-trigger
          data-from={arrow.from}
          data-to={arrow.to}
          data-kind={arrow.kind}
          data-lag={arrow.lagDays}
          data-late={arrow.late ? '' : undefined}
          data-loop={arrow.loop ? '' : undefined}
          tabindex={tabStopKey === arrow.key ? 0 : -1}
          aria-label={arrow.label}
          title={arrow.label}
          aria-haspopup="dialog"
          aria-expanded={selectedKey === arrow.key}
          class={[
            'absolute -translate-x-1/2 -translate-y-1/2 cursor-pointer rounded-full border px-1.5 py-0.5 text-[0.6875rem] leading-none font-medium whitespace-nowrap tabular-nums shadow-xs transition-opacity outline-none focus-visible:pointer-events-auto focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring',
            arrow.late || arrow.loop
              ? 'border-destructive/60 bg-background/95 text-destructive'
              : 'border-border bg-background/95 text-foreground',
            active && 'border-primary ring-1 ring-primary/40',
            shown ? 'pointer-events-auto opacity-100' : 'pointer-events-none opacity-0',
          ]}
          style:left="{arrow.labelX}rem"
          style:top="calc(var(--gantt-row) * {arrow.labelY})"
          onmouseenter={() => {
            onhover?.(arrow.key);
          }}
          onmouseleave={() => {
            if (hoveredKey === arrow.key) onhover?.(undefined);
          }}
          onfocus={(event) => {
            lastFocusedKey = arrow.key;
            onfocuskey?.(arrow.key);
            onfocused?.(event.currentTarget);
          }}
          onblur={() => {
            if (focusedKey === arrow.key) onfocuskey?.(undefined);
          }}
          onkeydown={(event) => {
            triggerKeys(event, index);
          }}
          onclick={(event) => {
            onselect?.(arrow, event.currentTarget);
          }}
        >
          <span aria-hidden="true">{arrow.badge || 'FS'}</span>
        </button>
      {:else if arrow.badge !== ''}
        <span
          role="img"
          data-arrow-badge
          data-from={arrow.from}
          data-to={arrow.to}
          data-kind={arrow.kind}
          data-lag={arrow.lagDays}
          aria-label={arrow.label}
          title={arrow.label}
          class={[
            'pointer-events-none absolute -translate-x-1/2 -translate-y-1/2 rounded-full border px-1.5 py-0.5 text-[0.6875rem] leading-none font-medium whitespace-nowrap tabular-nums shadow-xs',
            arrow.late || arrow.loop
              ? 'border-destructive/60 bg-background/95 text-destructive'
              : 'border-border bg-background/95 text-foreground',
          ]}
          style:left="{arrow.labelX}rem"
          style:top="calc(var(--gantt-row) * {arrow.labelY})"
        >
          <span aria-hidden="true">{arrow.badge}</span>
        </span>
      {:else}
        <span
          role="img"
          data-arrow-label
          data-from={arrow.from}
          data-to={arrow.to}
          aria-label={arrow.label}
          class="sr-only"
        ></span>
      {/if}
    {/each}
  </div>
{/if}
