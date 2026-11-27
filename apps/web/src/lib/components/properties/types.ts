/**
 * What the property components are given. They take values and callbacks
 * only: nothing here reads a store or talks to a server.
 */
import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
import type { PersonName } from '$lib/logic/person-names.ts';
import type { Regional } from '$lib/logic/regional.ts';
import type { UserNames } from '$lib/logic/users.ts';
import type {
  Card,
  PropertyColor,
  PropertyOption,
  PropertyTemplate,
  PropertyValue,
} from '$lib/logic/wire.ts';

/** A person who can be the value of a person property. */
export interface PropertyUser extends UserNames {
  id: string;
  /** The parts of the name, by any of which the person is found. */
  name?: PersonName;
}

/**
 * Changes to the options of a select property. The component only reports
 * them; changing the property's definition is up to the caller.
 */
export interface OptionCallbacks {
  /**
   * A person typed a name that no option has and asked for it. The caller
   * makes the option (its identifier and colour are the caller's to choose)
   * and returns it; the component then puts it into the value.
   */
  onCreateOption?: (name: string) => PropertyOption | Promise<PropertyOption>;
  onRenameOption?: (option: PropertyOption, name: string) => void;
  /** `color` is a colour name such as `brown`. */
  onRecolorOption?: (option: PropertyOption, color: PropertyColor) => void;
  onDeleteOption?: (option: PropertyOption) => void;
}

/** The reader: their language, their regional settings, and the current moment. */
export interface ReaderContext {
  /** BCP 47 tag of the language the app is shown in. */
  locale: string;
  /** Time zone, first day of the week and way of writing hours, from the account. */
  regional: Regional;
  /** The current moment; defaults to the clock. Decides what "today" is. */
  now?: number;
}

export interface PropertyValueProps extends OptionCallbacks, ReaderContext {
  template: PropertyTemplate;
  /** The card the value belongs to; created and updated times and people come from it. */
  card: Card;
  /** The stored value of the property on the card, if any. */
  value: PropertyValue | undefined;
  /**
   * `display` only shows the value (a table cell that is not being edited,
   * a badge on a kanban card); `edit` lets a person change it.
   */
  mode?: 'display' | 'edit';
  /** The reader may not change the value. */
  readOnly?: boolean;
  /** Show a word such as "Empty" where there is no value. */
  showEmptyPlaceholder?: boolean;
  /** Start in the editing state: the field focused, or the picker open. */
  startOpen?: boolean;
  /** The people a person property can be set to. */
  users?: readonly PropertyUser[];
  /** People a value may name though they are not offered: an author who left, say. */
  named?: readonly PropertyUser[];
  /** How pickers open. The caller decides; nothing here looks at the viewport. */
  presentation?: Presentation;
  /**
   * The new value to store. A cleared value is `''` (or `[]` for the kinds
   * that hold a list); a cleared date is `undefined`. A date is the wire
   * value that `formatDateValue` makes.
   */
  onChange?: (value: PropertyValue | undefined) => void;
  /**
   * The board's cards that are there, by identifier: what a relation
   * property picks from and shows. A card it names that is not here
   * (deleted) is not shown.
   */
  cards?: ReadonlyMap<string, Card>;
  /** Opens another card: a card a relation names is opened by pressing it. */
  onOpenCard?: (cardId: string) => void;
  /**
   * A relation is read as an order ("comes after"): a card that would close
   * a loop is not offered. A loop that is there already is only shown.
   */
  preventLoops?: boolean;
  /** Puts a text on the clipboard; defaults to the browser's clipboard. */
  copyText?: (text: string) => void | Promise<void>;
  /**
   * For a place that has one Tab stop for many values (a table): the
   * controls a value shows while it is only read, the link of an address
   * and its buttons, take this instead of a stop of their own and carry
   * `data-inner`, so that the place can open them to Tab for the value the
   * reader is on.
   */
  innerTabindex?: 0 | -1;
}
