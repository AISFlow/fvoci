/**
 * What the references of the bodies on screen name, for the reader: asked of
 * the backend once (`References.resolve`, all that is drawn in one turn in
 * one request), then held and kept current by the change messages of the
 * boards and wikis what is drawn is on. A card or a page that is renamed
 * shows its new title, one that is deleted, or whose board or wiki the reader
 * leaves, shows as what the reader may not see; the pages of a wiki that a
 * page was deleted from are asked again, as the pages below it went with it
 * and are not told of. When the connection was away, or the reader
 * comes back to a workspace, everything held there is asked again, and when a
 * board comes into the reader's sight, what they could not see is. What
 * could not be asked shows as what the reader may not see, and is asked
 * again a while later.
 *
 * An answer never puts back what a change heard while it was asked for
 * changed: the change is newer.
 *
 * One of these belongs to a `Workspace` (`referenceTitles`); what it holds is
 * kept by the workspace entered, where an identifier is asked.
 */
import { SvelteMap } from 'svelte/reactivity';

import { cardsNamed } from '../logic/card-numbers.ts';
import { cardIcon, type Card } from '../logic/wire.ts';

import { isBackendError, isCardBlock, type Change } from './backend.ts';
import type { BoardStore } from './board.svelte.ts';
import { REFERENCES_AT_ONCE, type LinkedBlock } from './references.ts';
import type { Paging } from './trash-reads.ts';
import type { Workspace } from './workspace.svelte.ts';

/** Where a reference leads. The app hands it in: it has the address bar. */
export interface ReferencePlaces {
  /** The address of what is named; none where the app has no screen for it. */
  href(linked: LinkedBlock): string | undefined;
  /** Shows what is named. */
  open(linked: LinkedBlock): void;
  /** The identifier of the card or the page an address of this workspace names. */
  named(address: string): string | undefined;
}

/** How long a search waits for the next letter before it asks. */
const SEARCH_WAIT_MS = 150;
/** How many cards of the open board are offered before anything is typed. */
const OFFERED_UNASKED = 8;
/** How long what could not be asked waits before it is asked again, the first time. */
const RETRY_FIRST_MS = 5_000;
/** The longest wait between two tries of what could not be asked. */
const RETRY_LONGEST_MS = 60_000;

/** The identifiers asked together in one turn, and the answer they wait for. */
interface Batch {
  workspaceId: string;
  ids: string[];
  answered: Promise<void>;
}

export class ReferenceTitles {
  readonly #workspace: Workspace;
  /** Where references lead, once the app has said. */
  places = $state.raw<ReferencePlaces>();
  /**
   * What is known, by workspace and identifier: `null` for what the reader may
   * not see. Whoever reads an entry where state is followed reads it again
   * when it changes.
   */
  readonly #known = new SvelteMap<string, LinkedBlock | null>();
  /** What is being asked. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- nothing that is drawn reads it
  readonly #pending = new Map<string, Promise<void>>();
  #batch: Batch | undefined;
  /** The boards whose changes are followed, each with what stops that. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- nothing that is drawn reads it
  readonly #watched = new Map<string, () => void>();
  #following = false;
  #stopSelf: (() => void) | undefined;
  #searches = 0;
  #latest: Promise<LinkedBlock[]> = Promise.resolve([]);
  /** The workspace that was entered when something was last asked. */
  #entered: string | undefined;
  /** Counts what happens to what is known: asking, and hearing of a change. */
  #ticks = 0;
  /** When a change to each entry was last heard (`#ticks`). */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- nothing that is drawn reads it
  readonly #changedAt = new Map<string, number>();
  #retryWait = RETRY_FIRST_MS;
  #retry: ReturnType<typeof setTimeout> | undefined;
  /** What could not be asked, by workspace and identifier, for the next try. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- nothing that is drawn reads it
  readonly #unanswered = new Set<string>();

  constructor(workspace: Workspace) {
    this.#workspace = workspace;
    workspace.onClose(() => {
      this.close();
    });
  }

  /** Stops all container/self subscriptions and pending retry timers on workspace close/sign-out. */
  close(): void {
    this.#stopSelf?.();
    this.#stopSelf = undefined;
    this.#following = false;
    this.#unwatchAll();
    if (this.#retry !== undefined) {
      clearTimeout(this.#retry);
      this.#retry = undefined;
    }
    this.#unanswered.clear();
  }

  /**
   * The workspace an identifier is asked in; a reader by a link has none. A
   * workspace that is entered again may have changed while another was shown,
   * and its boards were not followed meanwhile: what is held of it is asked
   * again, and the boards of the one left are no more followed.
   */
  #here(): string {
    const id = this.#workspace.info?.id ?? '';
    if (id === this.#entered) return id;
    const back = this.#entered !== undefined;
    this.#entered = id;
    if (back) {
      // Out of the drawing that noticed: following and unfollowing are no part of it.
      queueMicrotask(() => {
        this.#unwatchAll();
        void this.#ask(this.#knownHere(() => true));
      });
    }
    return id;
  }

  /**
   * What a reference shows: `undefined` while it is being asked (it is asked
   * by this), `null` for what the reader may not see. Read in an effect, it
   * runs again when the answer comes and whenever what it shows changes.
   */
  of(id: string): LinkedBlock | null | undefined {
    const key = `${this.#here()}/${id}`;
    if (!this.#known.has(key)) {
      void this.#ask([id]);
      return undefined;
    }
    const linked = this.#known.get(key);
    // What is drawn follows its board, from out of the drawing that reads it.
    if (linked && !this.#watched.has(linked.containerId)) {
      const { containerId } = linked;
      queueMicrotask(() => {
        this.#watch(containerId);
      });
    }
    return linked;
  }

  /** Whether the reader may see what the identifier names, asked if it is not known. */
  async seen(id: string): Promise<boolean> {
    const key = `${this.#here()}/${id}`;
    if (!this.#known.has(key)) await this.#ask([id]);
    return (this.#known.get(key) ?? null) !== null;
  }

  /** What names the block, a page of it; what is read is known from then on. */
  async to(blockId: string, paging?: Paging): Promise<LinkedBlock[]> {
    const workspaceId = this.#here();
    const asked = this.#tick();
    try {
      const found = await this.#workspace.backend.references.to(workspaceId, blockId, paging);
      this.#learn(workspaceId, found, asked);
      return found;
    } catch (error) {
      this.#failed(error);
      throw error;
    }
  }

  /**
   * What may be named whose title has the words of `term`. Asked once the
   * typing rests, and only the last term typed is: an earlier one is answered
   * as the last. Before anything is typed, the cards of `board` changed last;
   * without a board (in a page of a wiki), what the server has changed last.
   * What is offered is known from then on: taken, it is drawn without asking.
   */
  search(term: string, board?: Pick<BoardStore, 'realCards'>): Promise<LinkedBlock[]> {
    this.#searches += 1;
    const mine = this.#searches;
    const workspaceId = this.#here();
    const found = (async (): Promise<LinkedBlock[]> => {
      if (term.trim() === '' && board) {
        const recent = recentOf(board);
        this.#learn(workspaceId, recent, this.#tick());
        return recent;
      }
      await new Promise((resolve) => setTimeout(resolve, SEARCH_WAIT_MS));
      if (mine !== this.#searches) return this.#latest;
      try {
        const asked = this.#tick();
        const blocks = await this.#workspace.backend.references.search(workspaceId, term);
        this.#learn(workspaceId, blocks, asked);
        return mine === this.#searches ? blocks : await this.#latest;
      } catch (error) {
        this.#failed(error);
        return [];
      }
    })();
    this.#latest = found;
    return found;
  }

  /**
   * The cards of `board` but `around` that what was typed after `#` names
   * (`cardsNamed`): of what the board holds, without asking. What is offered
   * is known from then on: taken, it is drawn without asking.
   */
  onBoard(typed: string, board: Pick<BoardStore, 'realCards'>, around?: string): LinkedBlock[] {
    const workspaceId = this.#here();
    const others = board.realCards.filter((card) => card.id !== around);
    const found = cardsNamed(others, typed).map(linkedOf);
    this.#learn(workspaceId, found, this.#tick());
    return found;
  }

  #failed(error: unknown): void {
    // A session that is over signs out, whichever workspace is shown by then.
    if (isBackendError(error) && error.status === 401) this.#workspace.report('load', error);
    else if (!isBackendError(error)) console.error(error);
  }

  /** A new moment, after everything heard so far. */
  #tick(): number {
    this.#ticks += 1;
    return this.#ticks;
  }

  #learn(workspaceId: string, blocks: readonly LinkedBlock[], asked: number): void {
    for (const linked of blocks) this.#set(workspaceId, linked.id, linked, asked);
  }

  /** Holds what was answered when `asked`, unless a change heard since says otherwise. */
  #set(workspaceId: string, id: string, linked: LinkedBlock | null, asked: number): void {
    const key = `${workspaceId}/${id}`;
    if ((this.#changedAt.get(key) ?? 0) > asked) return;
    this.#known.set(key, linked);
  }

  #watch(containerId: string): void {
    if (this.#watched.has(containerId)) return;
    const stop = this.#workspace.backend.watchContainer(containerId, (change) => {
      this.#heard(change);
    });
    this.#watched.set(containerId, stop);
  }

  #unwatchAll(): void {
    for (const stop of this.#watched.values()) stop();
    this.#watched.clear();
  }

  /** Asks for the identifiers that are not being asked, with all that is asked this turn. */
  #ask(ids: readonly string[]): Promise<void> {
    const workspaceId = this.#here();
    this.#follow();
    const waited: Promise<void>[] = [];
    for (const id of ids) {
      const key = `${workspaceId}/${id}`;
      const asked = this.#pending.get(key);
      if (asked) {
        waited.push(asked);
        continue;
      }
      let batch = this.#batch;
      if (batch?.workspaceId !== workspaceId) {
        const made: Batch = { workspaceId, ids: [], answered: Promise.resolve() };
        made.answered = Promise.resolve().then(() => {
          if (this.#batch === made) this.#batch = undefined;
          return this.#read(made.workspaceId, made.ids);
        });
        this.#batch = batch = made;
      }
      batch.ids.push(id);
      this.#pending.set(key, batch.answered);
      waited.push(batch.answered);
    }
    return Promise.all(waited).then(() => undefined);
  }

  async #read(workspaceId: string, ids: readonly string[]): Promise<void> {
    let start = 0;
    try {
      for (; start < ids.length; start += REFERENCES_AT_ONCE) {
        const part = ids.slice(start, start + REFERENCES_AT_ONCE);
        const asked = this.#tick();
        const found = await this.#workspace.backend.references.resolve(workspaceId, part);
        for (const id of part) {
          this.#set(workspaceId, id, found.find((linked) => linked.id === id) ?? null, asked);
        }
      }
      this.#retryWait = RETRY_FIRST_MS;
    } catch (error) {
      this.#failed(error);
      // A refusal is the server's answer about the workspace, and the same next time.
      const again = !isBackendError(error) || error.status >= 500;
      this.#notAnswered(workspaceId, ids.slice(start), again);
    } finally {
      for (const id of ids) this.#pending.delete(`${workspaceId}/${id}`);
    }
  }

  /**
   * What could not be asked: shown as what the reader may not see (no title,
   * no link) rather than waited for, and, when the server or the way to it
   * failed (`again`), asked again a while later with whatever else failed
   * meanwhile, a longer while each time it fails again.
   */
  #notAnswered(workspaceId: string, ids: readonly string[], again: boolean): void {
    const asked = this.#tick();
    for (const id of ids) {
      const key = `${workspaceId}/${id}`;
      if (!this.#known.has(key)) this.#set(workspaceId, id, null, asked);
      if (again) this.#unanswered.add(key);
    }
    if (!again || this.#retry !== undefined) return;
    const wait = this.#retryWait;
    this.#retryWait = Math.min(wait * 2, RETRY_LONGEST_MS);
    this.#retry = setTimeout(() => {
      this.#retry = undefined;
      const prefix = `${this.#here()}/`;
      const ids = [...this.#unanswered]
        .filter((key) => key.startsWith(prefix))
        .map((key) => key.slice(prefix.length));
      this.#unanswered.clear();
      void this.#ask(ids);
    }, wait);
  }

  /** The identifiers known in the workspace entered that `wanted` picks. */
  #knownHere(wanted: (linked: LinkedBlock | null) => boolean): string[] {
    const prefix = `${this.#here()}/`;
    return [...this.#known]
      .filter(([key, linked]) => key.startsWith(prefix) && wanted(linked))
      .map(([key]) => key.slice(prefix.length));
  }

  /** Follows what is the reader's own: a connection that is back, a board that came into sight. */
  #follow(): void {
    if (this.#following) return;
    this.#following = true;
    this.#stopSelf = this.#workspace.backend.watchSelf((change) => {
      if (change.kind === 'resync') void this.#ask(this.#knownHere(() => true));
      else if (change.kind === 'container') void this.#ask(this.#knownHere((linked) => !linked));
      // A board the reader is taken off.
      else if (change.kind === 'containerDeleted') this.#heard(change);
    });
  }

  /** A change of a board that something known is on. */
  #heard(change: Change): void {
    const workspaceId = this.#here();
    if (change.kind === 'blocks') {
      for (const block of change.blocks) {
        const key = `${workspaceId}/${block.id}`;
        if (!this.#held(key) || !isCardBlock(block)) continue;
        const { id, title, containerId } = block;
        const icon = cardIcon(block);
        const number = block.number ?? this.#known.get(key)?.number;
        this.#changedAt.set(key, this.#tick());
        this.#known.set(key, {
          id,
          kind: 'card',
          title,
          icon,
          containerId,
          containerKind: 'board',
          ...(number === undefined ? {} : { number }),
        });
      }
    } else if (change.kind === 'pages') {
      for (const page of change.pages) {
        const key = `${workspaceId}/${page.id}`;
        if (!this.#held(key)) continue;
        const { id, title, icon, wikiId } = page;
        this.#changedAt.set(key, this.#tick());
        this.#known.set(key, {
          id,
          kind: 'page',
          title,
          icon,
          containerId: wikiId,
          containerKind: 'wiki',
        });
      }
    } else if (change.kind === 'units') {
      for (const unit of change.units) {
        if (unit.kind !== 'unit') continue;
        const key = `${workspaceId}/${unit.id}`;
        if (!this.#held(key)) continue;
        const { id, title, courseId } = unit;
        this.#changedAt.set(key, this.#tick());
        this.#known.set(key, {
          id,
          kind: 'unit',
          title,
          icon: '',
          containerId: courseId,
          containerKind: 'course',
        });
      }
    } else if (change.kind === 'blocksDeleted') {
      for (const id of change.blockIds) this.#unsee(`${workspaceId}/${id}`);
      // A page or chapter goes with those below it, and only it is told of: the rest are asked.
      const { containerId } = change;
      const nested = this.#knownHere(
        (linked) =>
          (linked?.kind === 'page' || linked?.kind === 'unit') &&
          linked.containerId === containerId,
      );
      if (nested.length > 0) void this.#ask(nested);
    } else if (change.kind === 'containerDeleted') {
      for (const id of this.#knownHere((linked) => linked?.containerId === change.containerId)) {
        this.#unsee(`${workspaceId}/${id}`);
      }
    }
  }

  /** Whether the entry is known or being asked: a change of it is newer than an answer on the way. */
  #held(key: string): boolean {
    return this.#known.has(key) || this.#pending.has(key);
  }

  #unsee(key: string): void {
    if (!this.#held(key) || this.#known.get(key) === null) return;
    this.#changedAt.set(key, this.#tick());
    this.#known.set(key, null);
  }
}

/** A card of a board the reader holds, as a reference shows it. */
function linkedOf(card: Card): LinkedBlock {
  return {
    id: card.id,
    kind: 'card',
    title: card.title,
    icon: cardIcon(card),
    containerId: card.containerId,
    containerKind: 'board',
    ...(card.number === undefined ? {} : { number: card.number }),
  };
}

/** The cards of the board that were changed last, as references show them; templates left out. */
function recentOf(board: Pick<BoardStore, 'realCards'>): LinkedBlock[] {
  return [...board.realCards]
    .sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id))
    .slice(0, OFFERED_UNASKED)
    .map(linkedOf);
}

const titles = new WeakMap<Workspace, ReferenceTitles>();

/** What the references shown in a workspace name: one for all its screens. */
export function referenceTitles(workspace: Workspace): ReferenceTitles {
  const held = titles.get(workspace);
  if (held) return held;
  const made = new ReferenceTitles(workspace);
  titles.set(workspace, made);
  return made;
}
