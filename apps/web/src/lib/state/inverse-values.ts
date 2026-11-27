/** Named values of a patch, including clears and board property definitions by identifier. */
type Patch = Record<string, unknown>;

/** The same value, whatever the order of the keys of its objects (the server sorts them). */
export function sameValue(first: unknown, second: unknown): boolean {
  return first === second || JSON.stringify(sorted(first)) === JSON.stringify(sorted(second));
}

function sorted(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sorted);
  if (typeof value !== 'object' || value === null) return value;
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map((key) => [key, sorted((value as Record<string, unknown>)[key])]),
  );
}

export class InverseValues {
  readonly #updated: string;
  readonly #deleted: string;

  constructor(updated: string, deleted: string) {
    this.#updated = updated;
    this.#deleted = deleted;
  }

  read(patch: Patch): Map<string, unknown> {
    const values = new Map<string, unknown>();
    for (const [key, value] of Object.entries(patch)) {
      if (key === this.#updated) {
        for (const [name, member] of Object.entries(value as Patch)) {
          values.set(JSON.stringify([key, name]), member);
        }
      } else if (key === this.#deleted || key === 'deletedCardProperties') {
        const updated = key === this.#deleted ? this.#updated : 'updatedCardProperties';
        for (const name of value as string[])
          values.set(JSON.stringify([updated, name]), undefined);
      } else if (key === 'updatedCardProperties') {
        for (const member of value as (Patch & { id: string })[]) {
          values.set(JSON.stringify([key, member.id]), member);
        }
      } else if (key === 'readAt') {
        continue;
      } else if (value !== undefined) {
        values.set(JSON.stringify([key]), value);
      }
    }
    return values;
  }

  put(patch: Patch, field: string, value: unknown): void {
    const [key, name] = JSON.parse(field) as [string, string?];
    if (name === undefined) {
      patch[key] = structuredClone(value);
      return;
    }
    const list = key === 'updatedCardProperties';
    const deleted = list ? 'deletedCardProperties' : this.#deleted;
    const cleared = ((patch[deleted] as string[] | undefined) ?? []).filter((id) => id !== name);
    if (value === undefined) cleared.push(name);
    patch[deleted] = cleared;
    if (list) {
      const members = (patch[key] as (Patch & { id: string })[] | undefined) ?? [];
      const at = members.findIndex((member) => member.id === name);
      const next = members.filter((member) => member.id !== name);
      if (value !== undefined)
        next.splice(at < 0 ? next.length : at, 0, structuredClone(value) as Patch & { id: string });
      patch[key] = next;
    } else {
      const members = { ...(patch[key] as Patch | undefined) };
      // eslint-disable-next-line @typescript-eslint/no-dynamic-delete -- a patch names a value by its key
      delete members[name];
      if (value !== undefined) members[name] = structuredClone(value);
      patch[key] = members;
    }
  }
}
