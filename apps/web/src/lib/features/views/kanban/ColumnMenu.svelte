<!--
  The "…" of a column: hide it, move it a place left or right, and, when the
  column is an option of a select property, rename it, give it another
  colour, or delete the option (after asking).

  Renaming happens in the heading of the column, so this only says that it
  was asked for. The colours open as a second surface at the same place.
-->
<script lang="ts">
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
  import CheckIcon from '@lucide/svelte/icons/check';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import EyeOffIcon from '@lucide/svelte/icons/eye-off';
  import PaletteIcon from '@lucide/svelte/icons/palette';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import TrashIcon from '@lucide/svelte/icons/trash-2';

  import { arrowChoice } from '$lib/components/arrow-choice.ts';
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import {
    OPTION_COLORS,
    optionColorName,
    optionFill,
  } from '$lib/components/option-chip/colors.ts';
  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { PropertyOption, PropertyTemplate } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';

  import { changeOption, deleteOption } from './board-data.ts';

  interface Props {
    board: BoardStore;
    groupBy: PropertyTemplate;
    option: PropertyOption;
    /** The name the column shows. */
    name: string;
    /** The column is an option of a select property: it has a name and a colour of its own. */
    isOption: boolean;
    /** The column can go a place to the left, to the right. Undefined: columns have no order to change. */
    canStep: { left: boolean; right: boolean } | undefined;
    /** The name of the column of cards without a value, for the question before deleting. */
    noValueName: string;
    onHide: () => void;
    onStep: (step: -1 | 1) => void;
    onRename: () => void;
  }

  let {
    board,
    groupBy,
    option,
    name,
    isOption,
    canStep,
    noValueName,
    onHide,
    onStep,
    onRename,
  }: Props = $props();

  const presentation = new PickerPresentation();
  let open = $state(false);
  let colours = $state(false);
  let confirming = $state(false);
  let holder = $state<HTMLElement | null>(null);

  const actions = $derived.by((): MenuAction[] => {
    const list: MenuAction[] = [
      { id: 'hide', label: m['BoardComponent.hide'](), icon: EyeOffIcon, run: onHide },
    ];
    if (isOption) {
      list.push(
        {
          id: 'rename',
          label: m.kanban_rename(),
          icon: PencilIcon,
          afterClose: onRename,
        },
        {
          id: 'color',
          label: m.kanban_change_color(),
          icon: PaletteIcon,
          afterClose: () => {
            colours = true;
          },
        },
      );
    }
    if (canStep) {
      list.push(
        {
          id: 'left',
          label: m.kanban_move_left(),
          icon: ArrowLeftIcon,
          disabled: !canStep.left,
          separated: true,
          run: () => {
            onStep(-1);
          },
        },
        {
          id: 'right',
          label: m.kanban_move_right(),
          icon: ArrowRightIcon,
          disabled: !canStep.right,
          run: () => {
            onStep(1);
          },
        },
      );
    }
    if (isOption) {
      list.push({
        id: 'delete',
        label: m['BoardComponent.delete'](),
        icon: TrashIcon,
        destructive: true,
        separated: true,
        afterClose: () => {
          confirming = true;
        },
      });
    }
    return list;
  });
</script>

<span bind:this={holder} class="relative block shrink-0">
  <ActionMenu bind:open title={m.kanban_column_menu({ name })} {actions}>
    {#snippet trigger(props: Record<string, unknown>)}
      <IconButton
        {...props}
        label={m.kanban_column_menu({ name })}
        data-column-menu={option.id}
        class="text-muted-foreground"
      >
        <EllipsisIcon aria-hidden="true" />
      </IconButton>
    {/snippet}
  </ActionMenu>

  {#if isOption}
    <PickerSurface
      bind:open={colours}
      presentation={presentation.current}
      title={m.kanban_change_color()}
      returnFocus={() => holder?.querySelector('button')}
    >
      {#snippet trigger({ props }: { props: Record<string, unknown> })}
        <span {...props} aria-hidden="true" class="pointer-events-none absolute inset-0"></span>
      {/snippet}
      <!-- Choosing a colour closes the list, so the arrow keys only move through it. -->
      <div
        role="radiogroup"
        aria-label={m.property_option_color()}
        class="flex flex-col"
        {@attach arrowChoice({ choose: false })}
      >
        {#each OPTION_COLORS as color (color)}
          <button
            type="button"
            role="radio"
            aria-checked={option.color === color}
            data-color={color}
            class="flex min-h-8 items-center gap-2 rounded-sm px-2 py-1 text-left text-sm group-data-[presentation=sheet]/surface:min-h-touch group-data-[presentation=sheet]/surface:text-base hover:bg-muted"
            onclick={() => {
              colours = false;
              if (option.color !== color)
                void changeOption(board, groupBy.id, option.id, { color });
            }}
          >
            <span
              class={['size-4 shrink-0 rounded-sm border border-foreground/15', optionFill(color)]}
              aria-hidden="true"
            ></span>
            <span class="flex-1">{optionColorName(color)}</span>
            {#if option.color === color}
              <CheckIcon class="size-4 shrink-0" aria-hidden="true" />
            {/if}
          </button>
        {/each}
      </div>
    </PickerSurface>

    <ConfirmDialog
      bind:open={confirming}
      title={m.kanban_delete_group_title({ name })}
      description={m.kanban_delete_group_description({
        property: groupBy.name,
        none: noValueName,
      })}
      confirmLabel={m['BoardComponent.delete']()}
      destructive
      onConfirm={() => void deleteOption(board, groupBy.id, option.id)}
    />
  {/if}
</span>
