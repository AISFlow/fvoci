/**
 * What the version being read is compared with, in the dialog of the
 * versions (BodyVersions.svelte): by default the version kept before it, so
 * that what it changed is what is seen; or the body now (what going back to
 * it would change); or any other version still kept. Or nothing: the version
 * as it is.
 *
 * Which of the first two, and whether to compare at all, is the reader's
 * choice for every version they read, and stays on the device. Another
 * version chosen to compare with stays only while the dialog is open (a
 * reading is made each time it opens), and gives way to the previous one
 * when it is the version being read or is no longer kept.
 *
 * The comparison is always from what is compared with to the version being
 * read: what is added is in the version, what is removed is in the other.
 */
import { comparable, compareBodies } from '$lib/editor/compare-bodies.ts';
import type { DocNode } from '$lib/editor/schema.ts';
import type { BodyDiff } from '$lib/logic/body-diff.ts';
import type { BodyVersion, BodyVersionContent } from '$lib/state/index.ts';

/** What the reader compares a version with, as they choose it for every version. */
export type CompareKind = 'previous' | 'current';

const KIND_KEY = 'fvoci.versions.compare';
const WHOLE_KEY = 'fvoci.versions.whole';

/**
 * What the version being read is compared with now: nothing (`whole`, or
 * `oldest`, the oldest version kept having none before it), the version
 * before it, the body now, or another version.
 */
export type CompareTarget =
  | { kind: 'whole' }
  | { kind: 'oldest' }
  | { kind: 'previous'; version: BodyVersion }
  | { kind: 'current' }
  | { kind: 'version'; version: BodyVersion };

/** What a comparison takes from the reading of the versions. */
export interface ComparedReading {
  /** The newest first. */
  readonly versions: BodyVersion[];
  readonly chosen: BodyVersion | undefined;
  readonly content: BodyVersionContent | undefined;
}

/** Where the bodies compared with come from. */
export interface CompareSource {
  /** The body of a version still kept. */
  read(version: number): Promise<DocNode | null>;
  /** The body now, with what was typed into it. */
  current(): Promise<DocNode | null>;
}

/** What `chosen` is compared with, of `versions` (the newest first), as the reader chose. */
export function targetOf(
  chosen: BodyVersion,
  versions: readonly BodyVersion[],
  choice: { kind: CompareKind; whole: boolean; pinned: number | undefined },
): CompareTarget {
  if (choice.whole) return { kind: 'whole' };
  if (choice.pinned !== undefined && choice.pinned !== chosen.version) {
    const pinned = versions.find((one) => one.version === choice.pinned);
    if (pinned) return { kind: 'version', version: pinned };
  }
  if (choice.kind === 'current') return { kind: 'current' };
  // The one kept before it, whatever its number: the server lets the oldest go.
  const previous = versions.find((one) => one.version < chosen.version);
  return previous ? { kind: 'previous', version: previous } : { kind: 'oldest' };
}

function remembered(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function remember(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not remembered: it still holds while the page is open.
  }
}

export class VersionComparing {
  kind = $state<CompareKind>(remembered(KIND_KEY) === 'current' ? 'current' : 'previous');
  whole = $state(remembered(WHOLE_KEY) === 'true');
  /** Another version chosen to compare with, by its number, while the dialog is open. */
  pinned = $state<number>();
  /**
   * `idle`: nothing to compare (no version read yet, or one shown as is);
   * `invalid`: the version read cannot be shown, and is not compared.
   */
  status = $state<'idle' | 'comparing' | 'ready' | 'failed' | 'invalid'>('idle');
  /** What changed, once compared. */
  diff = $state.raw<BodyDiff>();

  readonly #reading: ComparedReading;
  readonly #source: CompareSource;
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- read only here, never drawn
  readonly #bodies = new Map<number, Promise<DocNode | null>>();
  /** Counts the comparisons asked for: only the last one's answer is taken. */
  #asked = 0;

  /**
   * What the version being read is compared with when it is compared: also
   * while it is shown as it is, for the choice to say what it is.
   */
  readonly chosenTarget = $derived.by((): CompareTarget | undefined => {
    const chosen = this.#reading.chosen;
    if (!chosen) return undefined;
    return targetOf(chosen, this.#reading.versions, {
      kind: this.kind,
      whole: false,
      pinned: this.pinned,
    });
  });

  /** What the version being read is compared with now. */
  readonly target = $derived.by((): CompareTarget | undefined => {
    const chosen = this.#reading.chosen;
    if (!chosen) return undefined;
    return targetOf(chosen, this.#reading.versions, {
      kind: this.kind,
      whole: this.whole,
      pinned: this.pinned,
    });
  });

  constructor(reading: ComparedReading, source: CompareSource) {
    this.#reading = reading;
    this.#source = source;
  }

  /** Compares with the version before, the body now, or the version of that number. */
  compareWith(choice: CompareKind | number): void {
    if (typeof choice === 'number') this.pinned = choice;
    else {
      this.pinned = undefined;
      this.kind = choice;
      remember(KIND_KEY, choice);
    }
    this.showWhole(false);
  }

  /** Shows the version as it is (`true`), or compared. */
  showWhole(whole: boolean): void {
    this.whole = whole;
    remember(WHOLE_KEY, String(whole));
    void this.refresh();
  }

  /** Compares again, after the version read, its body, the list or the choice changed. */
  async refresh(): Promise<void> {
    const asked = (this.#asked += 1);
    const target = this.target;
    const content = this.#reading.content;
    this.diff = undefined;
    if (!target || !content || target.kind === 'whole' || target.kind === 'oldest') {
      this.status = 'idle';
      return;
    }
    if (!comparable(content.body)) {
      this.status = 'invalid';
      return;
    }
    this.status = 'comparing';
    try {
      const before =
        target.kind === 'current'
          ? await this.#source.current()
          : await this.#body(target.version.version);
      if (!comparable(before)) throw new Error('a body that cannot be shown');
      const diff = await compareBodies(before, content.body);
      if (asked !== this.#asked) return;
      this.diff = diff;
      this.status = 'ready';
    } catch {
      if (asked === this.#asked) this.status = 'failed';
    }
  }

  /** The body of a version, read once while the dialog is open. */
  #body(version: number): Promise<DocNode | null> {
    let body = this.#bodies.get(version);
    if (!body) {
      body = this.#source.read(version);
      // One that failed is asked for again next time.
      body.catch(() => this.#bodies.delete(version));
      this.#bodies.set(version, body);
    }
    return body;
  }
}
