/** The names of the kinds of property, as a menu shows them. */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { PropertyType } from '$lib/logic/wire.ts';

const NAMES: Record<PropertyType, () => string> = {
  text: m['PropertyType.Text'],
  number: m['PropertyType.Number'],
  email: m['PropertyType.Email'],
  phone: m['PropertyType.Phone'],
  url: m['PropertyType.Url'],
  select: m['PropertyType.Select'],
  multiSelect: m['PropertyType.MultiSelect'],
  date: m['PropertyType.Date'],
  person: m['PropertyType.Person'],
  multiPerson: m['PropertyType.MultiPerson'],
  checkbox: m['PropertyType.Checkbox'],
  createdTime: m['PropertyType.CreatedTime'],
  createdBy: m['PropertyType.CreatedBy'],
  updatedTime: m['PropertyType.UpdatedTime'],
  updatedBy: m['PropertyType.UpdatedBy'],
  relation: m.property_type_relation,
  file: m['PropertyType.Unknown'],
};

/** The name of a kind; a kind this app does not know is "Unknown". */
export function propertyTypeName(type: string): string {
  return (
    (NAMES as Record<string, (() => string) | undefined>)[type] ?? m['PropertyType.Unknown']
  )();
}
