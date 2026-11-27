<!--
  The value of one property of one card, for any kind of property: shown,
  and, where the reader may change it, edited.

  The kind is `template.type`. What a kind can do is looked up in
  PROPERTY_BEHAVIORS; a kind whose value comes from the card itself (created
  and updated time and person) is never editable. Everything the component
  needs is an argument: the people to choose from, the reader's language and
  regional settings, how pickers open.
-->
<script lang="ts" module>
  import type { Card } from '$lib/logic/wire.ts';

  const NO_CARDS: ReadonlyMap<string, Card> = new Map();
</script>

<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { DisplayContext } from '$lib/logic/format.ts';
  import { cardsClosingLoop, relationEdges } from '$lib/logic/dependencies.ts';
  import { propertyBehavior } from '$lib/logic/properties.ts';
  import { parseRelationValue, plainPropertyValue } from '$lib/logic/wire.ts';

  import CheckboxProperty from './kinds/CheckboxProperty.svelte';
  import DateProperty from './kinds/DateProperty.svelte';
  import PersonProperty from './kinds/PersonProperty.svelte';
  import RelationProperty from './kinds/RelationProperty.svelte';
  import SelectProperty from './kinds/SelectProperty.svelte';
  import TextProperty from './kinds/TextProperty.svelte';
  import UrlProperty from './kinds/UrlProperty.svelte';
  import type { PropertyValueProps } from './types.ts';

  let {
    template,
    card,
    value,
    mode = 'edit',
    readOnly = false,
    showEmptyPlaceholder = false,
    startOpen = false,
    users = [],
    named,
    locale,
    regional,
    now,
    presentation = 'popover',
    onChange,
    onCreateOption,
    onRenameOption,
    onRecolorOption,
    onDeleteOption,
    copyText,
    innerTabindex,
    cards = NO_CARDS,
    onOpenCard,
    preventLoops = false,
  }: PropertyValueProps = $props();

  const type = $derived(template.type);
  const behavior = $derived(propertyBehavior(type));
  const editable = $derived(mode === 'edit' && !readOnly && !behavior.isReadOnly);
  const open = $derived(startOpen && editable);

  /** The stored value as text or a list of texts; a date left on another kind reads as its key. */
  const plain = $derived(plainPropertyValue(value));
  /** For the kinds that hold one text. A list left behind by another kind is shown joined by commas. */
  const text = $derived(Array.isArray(plain) ? plain.join(',') : (plain ?? ''));
  /** For the kinds that hold identifiers: each one once. */
  const one = $derived(typeof plain === 'string' && plain !== '' ? [plain] : []);
  const many = $derived(Array.isArray(plain) ? [...new Set(plain)] : []);

  const empty = (message: () => string): string => (showEmptyPlaceholder ? message() : '');

  const context = $derived<DisplayContext>({
    locale,
    timeZone: regional.timeZone,
    hourCycle: regional.hourCycle,
    ...(now === undefined ? {} : { now }),
  });
  /** What the card itself says, for the kinds that have no stored value. */
  const fromCard = $derived.by(() => {
    const shown = behavior.displayValue(value, card, template, context);
    return typeof shown === 'string' ? shown : '';
  });

  /**
   * The cards a relation may not name without closing a loop; a picked one
   * among them is on a loop already. Worked out only where loops are
   * prevented, again when the cards change.
   */
  const closingLoop = $derived(
    preventLoops && type === 'relation'
      ? cardsClosingLoop(relationEdges(cards.values(), template.id), card.id)
      : undefined,
  );

  const change = (next: Parameters<NonNullable<PropertyValueProps['onChange']>>[0]): void => {
    if (editable) onChange?.(next);
  };
</script>

{#if type === 'text' || type === 'number' || type === 'email' || type === 'phone'}
  <TextProperty
    kind={type}
    name={template.name}
    value={text}
    {editable}
    placeholder={empty(m['PropertyValueElement.empty'])}
    startOpen={open}
    onChange={change}
  />
{:else if type === 'url'}
  <UrlProperty
    name={template.name}
    value={text}
    {editable}
    placeholder={empty(m['PropertyValueElement.empty'])}
    startOpen={open}
    onChange={change}
    {copyText}
    {innerTabindex}
  />
{:else if type === 'select' || type === 'multiSelect'}
  <SelectProperty
    name={template.name}
    options={template.options}
    ids={type === 'select' ? one : many}
    multiple={type === 'multiSelect'}
    {editable}
    placeholder={empty(m['PropertyValueElement.empty'])}
    {presentation}
    startOpen={open}
    onChange={change}
    {onCreateOption}
    {onRenameOption}
    {onRecolorOption}
    {onDeleteOption}
  />
{:else if type === 'date'}
  <DateProperty
    name={template.name}
    {value}
    {editable}
    placeholder={empty(m['DateRange.empty'])}
    {locale}
    {regional}
    {now}
    {presentation}
    startOpen={open}
    onChange={change}
  />
{:else if type === 'person' || type === 'multiPerson'}
  <PersonProperty
    name={template.name}
    ids={type === 'person' ? one : many}
    multiple={type === 'multiPerson'}
    {users}
    {named}
    {editable}
    placeholder={empty(m['ConfirmPerson.empty'])}
    {presentation}
    startOpen={open}
    onChange={change}
  />
{:else if type === 'relation'}
  <RelationProperty
    name={template.name}
    cardId={card.id}
    links={parseRelationValue(value)}
    {cards}
    {locale}
    excluded={closingLoop}
    {editable}
    placeholder={empty(m['PropertyValueElement.empty'])}
    {presentation}
    startOpen={open}
    onChange={change}
    {onOpenCard}
    {innerTabindex}
  />
{:else if type === 'checkbox'}
  <CheckboxProperty name={template.name} value={text} {editable} onChange={change} />
{:else if type === 'createdBy' || type === 'updatedBy'}
  {@const person = type === 'createdBy' ? card.createdBy : card.modifiedBy}
  <PersonProperty
    name={template.name}
    ids={person === '' ? [] : [person]}
    multiple={false}
    {users}
    {named}
    editable={false}
    placeholder=""
  />
{:else if type === 'createdTime' || type === 'updatedTime'}
  <TextProperty
    kind="text"
    name={template.name}
    value={fromCard}
    editable={false}
    placeholder=""
    onChange={change}
  />
{:else}
  <!-- A kind this app has no editor for: its stored text, read only. -->
  <TextProperty
    kind="text"
    name={template.name}
    value={text}
    editable={false}
    placeholder=""
    onChange={change}
  />
{/if}
