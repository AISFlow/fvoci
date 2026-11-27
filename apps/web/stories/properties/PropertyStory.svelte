<!--
  The stand a property is shown on in Storybook. It plays the caller: it
  keeps the stored value and the options, applies what the component reports,
  and writes every report on the screen. So a value really changes when it is
  edited here, with a keyboard or a finger, and what was stored can be read
  next to it.

  The same value is drawn twice: as it is edited (a row of a card's
  properties) and as it is only shown (a table cell, a badge on a card).
-->
<script lang="ts" module>
  import type { HourCycle } from '$lib/logic/format.ts';
  import type { Weekday } from '$lib/logic/regional.ts';
  import type { PropertyType, PropertyValue as StoredValue } from '$lib/logic/wire.ts';

  export interface PropertyStoryProps {
    type: PropertyType;
    /** The value the story starts with. */
    value?: StoredValue;
    readOnly?: boolean;
    startOpen?: boolean;
    /** `auto` follows the width of the viewport, like the app will. */
    presentation?: 'auto' | 'popover' | 'sheet';
    /** The time zone of the reader's account: not the one of this device. */
    timeZone?: string;
    hourCycle?: HourCycle;
    /** 1 is Monday, 7 is Sunday. */
    weekStart?: Weekday;
    /** How many options a select property has. */
    optionCount?: number;
  }
</script>

<script lang="ts">
  import { DEFAULT_OPTION_COLOR, OPTION_COLORS } from '$lib/components/option-chip/colors.ts';
  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import PropertyValue from '$lib/components/properties/PropertyValue.svelte';
  import type { PropertyUser } from '$lib/components/properties/types.ts';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import {
    createCard,
    createId,
    type PropertyColor,
    type PropertyOption,
  } from '$lib/logic/wire.ts';

  let {
    type,
    value,
    readOnly = false,
    startOpen = false,
    presentation = 'auto',
    timeZone = 'Asia/Seoul',
    hourCycle = 'h12',
    weekStart = 7,
    optionCount = 4,
  }: PropertyStoryProps = $props();

  const NAMES: Partial<Record<PropertyType, string>> = {
    text: '메모 Note',
    number: '예상 시간 Estimate',
    email: '담당자 메일 Email',
    phone: '연락처 Phone',
    url: '참고 링크 Link',
    select: '상태 Status',
    multiSelect: '태그 Tags',
    date: '마감 Due',
    person: '담당자 Assignee',
    multiPerson: '검토자 Reviewers',
    checkbox: '완료 Done',
    createdTime: '만든 시각 Created',
    createdBy: '만든 사람 Created by',
    updatedTime: '고친 시각 Updated',
    updatedBy: '고친 사람 Updated by',
  };

  const users: PropertyUser[] = [
    { id: 'example-member-1', username: 'example-member-1', displayName: '예시 구성원 1' },
    { id: 'example-member-2', username: 'example-member-2', displayName: '예시 구성원 2' },
    { id: 'example-member-3', username: 'example-member-3', displayName: 'Example member 3' },
    { id: 'example-member-4', username: 'example-member-4', displayName: 'Example member 4' },
    { id: 'example-member-5', username: 'example-member-5', displayName: '예시 구성원 5' },
    { id: 'example-member-6', username: 'example-bot', displayName: '' },
  ];

  const BASE_OPTIONS = ['할 일 To do', '진행 중 In progress', '검토 Review', '완료 Done'];
  // svelte-ignore state_referenced_locally (the story starts with these options and then owns them)
  let options = $state<PropertyOption[]>(
    Array.from({ length: optionCount }, (_, index) => ({
      id: `option-${index + 1}`,
      label: BASE_OPTIONS[index] ?? `선택지 ${index + 1}`,
      color: OPTION_COLORS[index % OPTION_COLORS.length] ?? DEFAULT_OPTION_COLOR,
    })),
  );

  // svelte-ignore state_referenced_locally (the story starts with this value and then owns it)
  let stored = $state.raw<StoredValue | undefined>(value);
  let log = $state<string[]>([]);

  const card = createCard();
  card.createdBy = 'example-member-1';
  card.modifiedBy = 'example-member-2';
  // Half past nine in the evening of 9 November 2026 in Seoul, and 01:05 the next night.
  card.createdAt = Date.UTC(2026, 10, 9, 12, 30);
  card.updatedAt = Date.UTC(2026, 10, 10, 16, 5);

  const template = $derived({ id: 'property', name: NAMES[type] ?? type, type, options });
  const regional = $derived({ timeZone, weekStart, hourCycle });
  const viewport = new PickerPresentation();
  const surface = $derived(presentation === 'auto' ? viewport.current : presentation);
  const locale = getLocale();

  const show = (detail: unknown): string =>
    detail === undefined ? 'undefined' : JSON.stringify(detail);

  function report(what: string, detail: unknown): void {
    log = [`${what} ${show(detail)}`, ...log].slice(0, 8);
  }

  function replaceOption(id: string, change: Partial<PropertyOption>): void {
    options = options.map((option) => (option.id === id ? { ...option, ...change } : option));
  }
</script>

<div class="grid max-w-xl gap-6">
  <section class="grid gap-2">
    <h2 class="text-xs font-medium text-muted-foreground">편집 Edit</h2>
    <div class="grid grid-cols-[9rem_minmax(0,1fr)] items-start gap-2">
      <div class="py-1.5 text-muted-foreground">{template.name}</div>
      <PropertyValue
        {template}
        {card}
        value={stored}
        {readOnly}
        {startOpen}
        showEmptyPlaceholder
        {users}
        {locale}
        {regional}
        presentation={surface}
        onChange={(next: StoredValue | undefined) => {
          stored = next;
          report('onChange', next);
        }}
        onCreateOption={(name: string) => {
          const option = { id: createId('block'), label: name, color: DEFAULT_OPTION_COLOR };
          options = [...options, option];
          report('onCreateOption', name);
          return option;
        }}
        onRenameOption={(option: PropertyOption, name: string) => {
          replaceOption(option.id, { label: name });
          report('onRenameOption', [option.label, name]);
        }}
        onRecolorOption={(option: PropertyOption, color: PropertyColor) => {
          replaceOption(option.id, { color });
          report('onRecolorOption', [option.label, color]);
        }}
        onDeleteOption={(option: PropertyOption) => {
          options = options.filter((other) => other.id !== option.id);
          report('onDeleteOption', option.label);
        }}
      />
    </div>
  </section>

  <section class="grid gap-2">
    <h2 class="text-xs font-medium text-muted-foreground">표시 Display</h2>
    <div class="w-56 rounded-md border">
      <PropertyValue {template} {card} value={stored} mode="display" {users} {locale} {regional} />
    </div>
  </section>

  <section class="grid gap-1 text-xs text-muted-foreground">
    <div>
      저장된 값 Stored value · 계정 시간대 Account time zone: {timeZone} · {surface}
    </div>
    <pre
      class="overflow-x-auto rounded-md bg-muted p-2 font-mono text-foreground"
      data-testid="stored">{show(stored)}</pre>
    <ol class="m-0 grid list-none gap-0.5 p-0 font-mono" aria-label="Callbacks" data-testid="log">
      {#each log as line, index (log.length - index)}
        <li class="truncate">{line}</li>
      {/each}
    </ol>
  </section>
</div>
