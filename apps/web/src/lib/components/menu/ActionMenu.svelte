<!--
  A menu of commands: the dropdown next to what opened it, or on a compact
  page a bottom sheet of tall lines. Which one is the `presentation`
  argument; without it the stage of the page decides
  (lib/layout/presentation.ts, the one place to change the rule).

  The sheet names what the menu is about (`title`, and `subtitle` for the
  card or board it belongs to), because it opens away from what was pressed.

  A line with an address (`href`) is a link, in either: it can be opened in
  a new tab (a click or Enter with Ctrl, ⌘ or Shift, the middle button), the
  one marked is the page shown (`aria-current`), and its whole label is its
  tooltip where the dropdown cuts it (`cutTitle`). The sheet's lines wrap
  instead, and have none: a finger finds no tooltip.

  A line may begin a group, named above it (`heading`): its lines say the
  group in their `ariaLabel`, as the heading is only shown.

  A line may end in tools (`MenuTool`), commands about its thing that leave
  the menu open, and `status` says what they did. Their keys (Alt and an
  arrow, say) run them from the line, which keeps the focus wherever it
  goes; the arrows alone still go from line to line. In the dropdown their
  buttons are out of the way of the arrows (not Bits UI's items), for a
  pointer and a screen reader's own reading; in the sheet Tab reaches them
  too.
-->
<script lang="ts">
  import CheckIcon from '@lucide/svelte/icons/check';
  import { tick, type Snippet } from 'svelte';

  import { overlayPresentation, type OverlayPresentation } from '$lib/layout/presentation.ts';
  import { Opener } from '$lib/focus.ts';
  import { insets, page } from '$lib/layout/room.svelte.ts';
  import { isApplePlatform } from '$lib/keys.ts';
  import { cutTitle } from '$lib/components/cut-title.ts';
  import * as DropdownMenu from '$lib/ui/dropdown-menu/index.js';

  import BottomSheet from '../sheet/BottomSheet.svelte';

  import { followedHere, type MenuAction, type MenuTool } from './actions.ts';

  interface Props {
    open?: boolean;
    title: string;
    subtitle?: string;
    actions: readonly MenuAction[];
    align?: 'start' | 'end';
    presentation?: OverlayPresentation;
    /** What the tools of a line did, for a screen reader: said while the menu is open. */
    status?: string;
    /** The element that opens it. Spread `props` on a button, before its own attributes. */
    trigger: Snippet<[Record<string, unknown>]>;
    /** Where the focus goes once the menu has closed, in place of a trigger nobody can focus. */
    returnFocus?: () => HTMLElement | null | undefined;
  }

  let {
    open = $bindable(false),
    title,
    subtitle,
    actions,
    align = 'end',
    presentation,
    status,
    trigger,
    returnFocus,
  }: Props = $props();

  const shown = $derived(presentation ?? overlayPresentation(page.stage));
  /** Lines with tools at their ends: a wider dropdown, for their names. */
  const withTools = $derived(actions.some((action) => action.tools));

  /**
   * Where a line of the dropdown is among its lines, with tools at their ends:
   * said as one of those the arrows go to, its tools not counted.
   */
  function linePlace(index: number): Record<string, number> {
    return withTools ? { 'aria-posinset': index + 1, 'aria-setsize': actions.length } : {};
  }
  const opener = new Opener();
  let dropdownTrigger = $state<HTMLElement | null>(null);
  /** The `afterClose` of the line chosen, for once the menu has closed. */
  let afterClose: (() => void) | undefined;

  function choose(action: MenuAction): void {
    open = false;
    afterClose = action.afterClose;
    action.run?.();
  }

  /** Once the menu has closed and the focus is back: what the line chosen left for then. */
  function closed(): void {
    const next = afterClose;
    afterClose = undefined;
    next?.();
  }

  /** The dropdown closed: the focus where `returnFocus` says, or on the trigger before `afterClose`. */
  function closedDropdown(event: Event): void {
    const back = returnFocus?.() ?? (afterClose ? dropdownTrigger : null);
    if (back?.isConnected) {
      event.preventDefault();
      back.focus({ preventScroll: true });
    }
    closed();
  }

  /** A link's line pressed: answered here, or (a new tab or window asked for) left to the browser. */
  function follow(event: MouseEvent, action: MenuAction): void {
    if (!followedHere(event)) return;
    event.preventDefault();
    action.run?.();
  }

  /** Names this menu's list in the page, for its lines to be found again once they move. */
  const uid = $props.id();

  /**
   * Runs a tool, the focus then on its line (`stay` 'line') or with the line's
   * tools ('tool'): on this one, or, it being disabled now (at an end), the other.
   */
  async function useTool(line: MenuAction, tool: MenuTool, stay: 'line' | 'tool'): Promise<void> {
    if (tool.disabled) return;
    tool.run();
    await tick();
    const row = document.querySelector(
      `[data-menu="${CSS.escape(uid)}"] [data-line="${CSS.escape(line.id)}"]`,
    );
    const lineItem = row?.querySelector<HTMLElement>(
      `[data-action="${CSS.escape(line.id)}"]:not([data-line-tool])`,
    );
    const usable = [...(row?.querySelectorAll<HTMLElement>('[data-line-tool]') ?? [])].filter(
      (each) => !each.matches(':disabled'),
    );
    const tools = stay === 'tool' ? usable : [];
    (tools.find((each) => each.dataset.action === tool.id) ?? tools[0] ?? lineItem)?.focus();
  }

  /** Whether `keys` (as `aria-keyshortcuts` writes them: `Alt+ArrowUp`) are what was pressed. */
  function pressed(event: KeyboardEvent, keys: string): boolean {
    const parts = keys.split('+');
    const held = (name: string): boolean => parts.slice(0, -1).includes(name);
    return (
      event.key === parts.at(-1) &&
      event.altKey === held('Alt') &&
      event.ctrlKey === held('Control') &&
      event.metaKey === held('Meta') &&
      event.shiftKey === held('Shift')
    );
  }

  /** The keys of a line's tools that do something there, for a screen reader to name with it. */
  function lineKeys(action: MenuAction): string | undefined {
    const keys = (action.tools ?? []).flatMap((tool) => (tool.disabled ? [] : [tool.keys]));
    return keys.length > 0 ? keys.join(' ') : undefined;
  }

  /** The keys of a tool pressed on its line: the tool runs there. Whether they were. */
  function toolKeys(event: KeyboardEvent, action: MenuAction): boolean {
    const tool = action.tools?.find((each) => pressed(event, each.keys));
    if (!tool) return false;
    // Not the menu's as well, whose arrows would go on to another line.
    event.preventDefault();
    event.stopPropagation();
    void useTool(action, tool, 'line');
    return true;
  }

  /** A tool's keys in its tooltip: ⌥ on Apple's systems, the arrows drawn. */
  function keysLabel(keys: string): string {
    return keys
      .replace('Alt+', isApplePlatform() ? '⌥' : 'Alt+')
      .replace('ArrowUp', '↑')
      .replace('ArrowDown', '↓');
  }

  const SHEET_LINE =
    'flex min-h-9 w-full items-center gap-3 rounded-lg px-3 text-left hover:bg-muted active:bg-muted disabled:opacity-50 roomy:min-h-12 roomy:text-base';
</script>

{#snippet sheetLine(action: MenuAction)}
  {@const Icon = action.icon}
  {#if Icon}<Icon class="size-5 shrink-0" />{/if}
  <!-- Wrapped, not cut: a sheet is read by touch, where no tooltip shows. -->
  <span class="min-w-0 flex-1 py-1 break-words">{action.label}</span>
  {#if action.hint}
    <span class="shrink-0 text-sm text-muted-foreground">{action.hint}</span>
  {/if}
  {#if action.checked}
    <CheckIcon class="size-5 shrink-0" aria-hidden="true" />
  {/if}
{/snippet}

{#snippet dropdownLine(action: MenuAction)}
  {@const Icon = action.icon}
  {#if Icon}<Icon />{/if}
  <span class="min-w-0 flex-1 truncate">{action.label}</span>
  {#if action.hint}
    <span class="shrink-0 text-xs text-muted-foreground">{action.hint}</span>
  {/if}
  {#if action.checked}
    <CheckIcon aria-hidden="true" />
  {/if}
{/snippet}

{#snippet dropdownItem(action: MenuAction, index: number, row = '')}
  {#if action.href === undefined}
    <DropdownMenu.Item
      {...linePlace(index)}
      variant={action.destructive ? 'destructive' : 'default'}
      disabled={action.disabled}
      aria-label={action.ariaLabel}
      aria-keyshortcuts={lineKeys(action)}
      data-action={action.id}
      class="roomy:min-h-touch roomy:gap-2.5 roomy:px-2.5 {row}"
      onkeydown={(event: KeyboardEvent) => {
        toolKeys(event, action);
      }}
      onSelect={() => {
        afterClose = action.afterClose;
        action.run?.();
      }}
    >
      {@render dropdownLine(action)}
    </DropdownMenu.Item>
  {:else}
    {@const href = action.href}
    <!-- Enter is a click on the link (Bits UI): it runs, and the menu closes. -->
    <!-- With a key that asks for a new tab or window, Enter is the browser's: its click
         carries the key, and goes to the browser as a click of the mouse does. -->
    <DropdownMenu.Item
      aria-label={action.ariaLabel}
      data-action={action.id}
      class="roomy:min-h-touch roomy:gap-2.5 roomy:px-2.5 {row}"
    >
      {#snippet child({ props }: { props: Record<string, unknown> })}
        {@const select = props.onclick as ((event: MouseEvent) => void) | undefined}
        {@const keys = props.onkeydown as ((event: KeyboardEvent) => void) | undefined}
        <a
          {...props}
          {...linePlace(index)}
          {href}
          aria-current={action.checked ? 'page' : undefined}
          aria-keyshortcuts={lineKeys(action)}
          {@attach cutTitle(action.label)}
          onclick={(event) => {
            select?.(event);
            follow(event, action);
          }}
          onkeydown={(event) => {
            if (toolKeys(event, action)) return;
            if (event.key === 'Enter' && (event.ctrlKey || event.metaKey || event.shiftKey)) return;
            keys?.(event);
          }}
        >
          {@render dropdownLine(action)}
        </a>
      {/snippet}
    </DropdownMenu.Item>
  {/if}
{/snippet}

{#if shown === 'sheet'}
  {@render trigger({
    'aria-haspopup': 'dialog',
    'aria-expanded': open,
    // In the capture phase: a trigger may put its own `onclick` in place of this one's.
    onclickcapture: (event: MouseEvent) => {
      opener.remember(event);
    },
    onclick: () => {
      open = true;
    },
  })}
  <BottomSheet
    bind:open
    {title}
    {subtitle}
    returnFocus={() => returnFocus?.() ?? opener.take()}
    onClosed={closed}
  >
    <ul class="px-2 pb-2" role="menu" aria-label={title} data-menu={uid}>
      {#each actions as action (action.id)}
        <li
          role="none"
          data-line={action.tools ? action.id : undefined}
          class={[action.separated && 'mt-1 border-t pt-1', action.tools && 'flex items-center']}
        >
          {#if action.heading}
            <span
              aria-hidden="true"
              class="block px-3 pt-2 pb-1 text-xs font-medium text-muted-foreground"
            >
              {action.heading}
            </span>
          {/if}
          {#if action.href === undefined}
            <button
              type="button"
              role="menuitem"
              aria-label={action.ariaLabel}
              aria-keyshortcuts={lineKeys(action)}
              data-action={action.id}
              disabled={action.disabled}
              class={[SHEET_LINE, action.destructive && 'text-destructive']}
              onclick={() => {
                choose(action);
              }}
              onkeydown={(event) => {
                toolKeys(event, action);
              }}
            >
              {@render sheetLine(action)}
            </button>
          {:else}
            <a
              href={action.href}
              role="menuitem"
              aria-label={action.ariaLabel}
              aria-current={action.checked ? 'page' : undefined}
              aria-keyshortcuts={lineKeys(action)}
              data-action={action.id}
              class={SHEET_LINE}
              onclick={(event) => {
                open = false;
                follow(event, action);
              }}
              onkeydown={(event) => {
                toolKeys(event, action);
              }}
            >
              {@render sheetLine(action)}
            </a>
          {/if}
          {#each action.tools ?? [] as tool (tool.id)}
            {@const Icon = tool.icon}
            <button
              type="button"
              role="menuitem"
              aria-label={tool.label}
              title="{tool.label} ({keysLabel(tool.keys)})"
              data-action={tool.id}
              data-line-tool
              disabled={tool.disabled}
              class="grid size-9 shrink-0 place-items-center rounded-lg text-muted-foreground hover:bg-muted active:bg-muted disabled:opacity-50 roomy:size-12"
              onclick={() => void useTool(action, tool, 'tool')}
            >
              <Icon class="size-5" />
            </button>
          {/each}
        </li>
      {/each}
    </ul>
    {#if status !== undefined}
      <span class="sr-only" aria-live="polite">{status}</span>
    {/if}
  </BottomSheet>
{:else}
  <DropdownMenu.Root bind:open>
    <DropdownMenu.Trigger bind:ref={dropdownTrigger}>
      {#snippet child({ props }: { props: Record<string, unknown> })}
        {@render trigger(props)}
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content
      {align}
      collisionPadding={insets.collisionPadding}
      aria-label={title}
      data-menu={uid}
      data-testid="dropdown"
      onCloseAutoFocus={closedDropdown}
      class={[
        withTools ? 'w-72' : 'w-60',
        'max-w-[calc(100vw-1rem-var(--safe-left)-var(--safe-right))]',
      ]}
    >
      {#each actions as action, index (action.id)}
        {#if action.separated}
          <DropdownMenu.Separator />
        {/if}
        {#if action.heading}
          <DropdownMenu.Label aria-hidden="true">{action.heading}</DropdownMenu.Label>
        {/if}
        {#if action.tools}
          <!-- One row: the line, and the buttons of its tools after it, items of the menu
               that its arrows pass over (not Bits UI's): the keys are the line's. -->
          <div role="none" class="flex items-center gap-0.5" data-line={action.id}>
            {@render dropdownItem(action, index, 'min-w-0 flex-1')}
            {#each action.tools as tool (tool.id)}
              {@const Icon = tool.icon}
              <button
                type="button"
                role="menuitem"
                tabindex="-1"
                aria-label={tool.label}
                title="{tool.label} ({keysLabel(tool.keys)})"
                data-action={tool.id}
                data-line-tool
                disabled={tool.disabled}
                class="grid size-7 shrink-0 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-accent-foreground disabled:pointer-events-none disabled:opacity-50 roomy:size-touch [&_svg]:size-4"
                onpointerdown={(event) => {
                  // The focus stays where it is, and goes to the line once it has moved.
                  event.preventDefault();
                }}
                onclick={() => void useTool(action, tool, 'line')}
              >
                <Icon />
              </button>
            {/each}
          </div>
        {:else}
          {@render dropdownItem(action, index)}
        {/if}
      {/each}
    </DropdownMenu.Content>
  </DropdownMenu.Root>
  {#if status !== undefined}
    <!-- Out of the menu, which has nothing but its items. -->
    <span class="sr-only" aria-live="polite">{status}</span>
  {/if}
{/if}
