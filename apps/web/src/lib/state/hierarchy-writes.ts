/**
 * Optimistic hierarchy write queue shared by tree-structured containers
 * (`WikiStore` pages and `CourseStore` chapters/units): creation, subtree
 * deletion, duplication, restoration, title edits, and serialized moves with
 * undo/redo and out-of-order echo reconciliation.
 */
import { isPlaced, moved, movesAnywhere, placeOf, type PagePlace } from '../logic/page-moves.ts';
import { withDescendants } from '../logic/page-tree.ts';

import { isBackendError } from './backend.ts';
import { unlessGone } from './wiki-parts.ts';
import type { Workspace } from './workspace.svelte.ts';
import { PendingWrites } from './write.ts';

export interface HierarchyNode {
  readonly id: string;
  readonly parentId: string;
  readonly title: string;
  readonly order: string;
  readonly updatedAt: number;
}

export interface OpenHierarchyNode {
  readonly id: string;
  readonly fallbackTitle: string;
  gone(by: 'others' | 'reader', title: string): void;
}

export interface HierarchyWriteHost<
  TNode extends HierarchyNode,
  TDetail extends TNode = TNode,
  TDraft = unknown,
> {
  readonly workspace: Workspace;
  nodes(): TNode[];
  setNodes(nodes: TNode[]): void;
  openNode(): OpenHierarchyNode | undefined;
  takeNode(node: TNode): void;
  reread(): void;
  create(draft: TDraft): Promise<TDetail>;
  hasBody(nodeId: string): Promise<boolean>;
  remove(nodeId: string): Promise<void>;
  restore(nodeId: string): Promise<TNode>;
  move(nodeId: string, to: PagePlace): Promise<TNode>;
  duplicate?: (nodeId: string, title: string) => Promise<TDetail>;
  patchTitle?: (nodeId: string, title: string) => Promise<TNode>;
}

export class HierarchyWriteQueue<
  TNode extends HierarchyNode,
  TDetail extends TNode = TNode,
  TDraft = unknown,
> {
  readonly #host: HierarchyWriteHost<TNode, TDetail, TDraft>;
  /** Nodes with an HTTP delete currently in flight, mapped to the open node title when started. */
  readonly #inFlightDeleting = new Map<string, string>();
  /**
   * Nodes deleted by this reader whose WS `blocksDeleted` echo has not yet
   * arrived (fixes W1.3: WS echoes arriving after HTTP 204 are still recognized
   * as `'reader'` rather than `'others'` without fragile `setTimeout(..., 0)`).
   */
  readonly #ownDeleted = new Map<string, string>();
  readonly #leaving = new Set<Promise<void>>();
  #forgettings = 0;
  readonly #forgottenAt = new Map<string, { at: number; updatedAt: number }>();
  readonly #moving = new Map<string, number>();
  readonly #pending = new PendingWrites<TNode>();
  readonly #unmoved = new Map<string, TNode>();
  #queue: Promise<unknown> = Promise.resolve();
  #queued = 0;

  constructor(host: HierarchyWriteHost<TNode, TDetail, TDraft>) {
    this.#host = host;
  }

  #node(nodeId: string | undefined): TNode | undefined {
    return this.#host.nodes().find((node) => node.id === nodeId);
  }

  isDeleting(nodeId: string): boolean {
    return this.#inFlightDeleting.has(nodeId) || this.#ownDeleted.has(nodeId);
  }

  get forgettings(): number {
    return this.#forgettings;
  }

  forgottenSince(mark: number, nodeId: string): boolean {
    return (this.#forgottenAt.get(nodeId)?.at ?? 0) > mark;
  }

  forget(nodeIds: readonly string[], by: 'others' | 'reader' | 'held'): void {
    const gone = nodeIds.flatMap((nodeId) => [...withDescendants(this.#host.nodes(), nodeId)]);
    const allIds = [...new Set([...nodeIds, ...gone])];
    this.#forgettings += 1;
    for (const nodeId of allIds) {
      const shownAt = this.#node(nodeId)?.updatedAt ?? -Infinity;
      const updatedAt = Math.max(shownAt, this.#forgottenAt.get(nodeId)?.updatedAt ?? -Infinity);
      this.#forgottenAt.set(nodeId, { at: this.#forgettings, updatedAt });
    }
    const shown = this.#host.openNode();
    const ownId = allIds.find(
      (nodeId) => this.#inFlightDeleting.has(nodeId) || this.#ownDeleted.has(nodeId),
    );
    const ownTitle =
      ownId === undefined
        ? undefined
        : (this.#inFlightDeleting.get(ownId) ?? this.#ownDeleted.get(ownId));
    if (by !== 'held') {
      for (const nodeId of allIds) {
        if (!this.#inFlightDeleting.has(nodeId)) this.#ownDeleted.delete(nodeId);
      }
    }
    const title =
      ownId === undefined ? (this.#node(shown?.id)?.title ?? shown?.fallbackTitle) : ownTitle;
    this.#host.setNodes(this.#host.nodes().filter((node) => !gone.includes(node.id)));
    if (by === 'held' || !shown || !gone.includes(shown.id)) return;
    shown.gone(ownId === undefined ? by : 'reader', title ?? '');
  }

  admits(node: TNode): boolean {
    const forgotten = this.#forgottenAt.get(node.id);
    if (forgotten && node.updatedAt <= forgotten.updatedAt) return false;
    if (node.parentId !== '' && this.#forgottenAt.has(node.parentId)) {
      if (this.#node(node.id)) this.forget([node.id], 'others');
      const kept = this.#forgottenAt.get(node.id)?.updatedAt ?? -Infinity;
      this.#forgottenAt.set(node.id, {
        at: this.#forgettings,
        updatedAt: Math.max(kept, node.updatedAt),
      });
      return false;
    }
    this.#forgottenAt.delete(node.id);
    this.#ownDeleted.delete(node.id);
    return true;
  }

  remember(nodes: readonly TNode[]): void {
    for (const node of nodes) {
      this.#forgottenAt.delete(node.id);
      this.#ownDeleted.delete(node.id);
    }
  }

  admitted(nodes: readonly TNode[], mark: number): TNode[] {
    const byId = new Map(nodes.map((node) => [node.id, node]));
    const gone = (node: TNode): boolean => {
      const seen = new Set<string>();
      for (let at: TNode | undefined = node; at && !seen.has(at.id); at = byId.get(at.parentId)) {
        seen.add(at.id);
        if (this.forgottenSince(mark, at.id)) return true;
        if (at.parentId !== '' && !byId.has(at.parentId) && this.#forgottenAt.has(at.parentId)) {
          return true;
        }
      }
      return false;
    };
    return nodes.filter((node) => !gone(node));
  }

  async deletionsAnswered(): Promise<void> {
    await Promise.allSettled([...this.#leaving]);
  }

  async #deleteNode(
    nodeId: string,
    subtreeIds: readonly string[],
    title = this.#node(this.#host.openNode()?.id)?.title ?? '',
  ): Promise<void> {
    const tracked = subtreeIds.length > 0 ? subtreeIds : [nodeId];
    for (const id of tracked) {
      this.#inFlightDeleting.set(id, title);
      this.#ownDeleted.set(id, title);
    }
    const leaving = this.#host.remove(nodeId);
    this.#leaving.add(leaving);
    try {
      await leaving;
    } catch (error) {
      if (!isBackendError(error) || error.status !== 404) {
        for (const id of tracked) this.#ownDeleted.delete(id);
      }
      throw error;
    } finally {
      this.#leaving.delete(leaving);
      for (const id of tracked) this.#inFlightDeleting.delete(id);
    }
  }

  async create(draft: TDraft, stepName: string): Promise<TDetail | undefined> {
    let kept: TDetail | undefined;
    try {
      return await this.#host.workspace.undo.perform(
        async () => {
          if (kept) {
            const again = kept;
            kept = undefined;
            return again;
          }
          const made = await this.#host.create(draft);
          this.#host.takeNode(made);
          return made;
        },
        async (made) => {
          if (await this.#grownSince(made.id)) {
            kept = made;
            return;
          }
          const subtree = [...withDescendants(this.#host.nodes(), made.id)];
          await unlessGone(() => this.#deleteNode(made.id, subtree));
          this.forget([made.id], 'reader');
        },
        stepName,
      );
    } catch (error) {
      this.#host.workspace.report('save', error);
      return undefined;
    }
  }

  async #grownSince(nodeId: string): Promise<boolean> {
    if (this.#host.nodes().some((node) => node.parentId === nodeId)) return true;
    try {
      return await this.#host.hasBody(nodeId);
    } catch (error) {
      if (isBackendError(error) && error.status === 404) return false;
      throw error;
    }
  }

  async remove(nodeId: string, stepName: string): Promise<boolean> {
    try {
      await this.#host.workspace.undo.perform(
        () => this.#deleteShown(nodeId),
        () => unlessGone(() => this.restore(nodeId)),
        stepName,
      );
      return true;
    } catch (error) {
      this.#host.workspace.report('save', error);
      return false;
    }
  }

  async #deleteShown(nodeId: string): Promise<void> {
    const before = this.#host.nodes();
    const gone = withDescendants(before, nodeId);
    const title = this.#node(this.#host.openNode()?.id)?.title ?? '';
    const marks = [...gone].map((id) => [id, this.#forgottenAt.get(id)] as const);
    this.forget([nodeId], 'held');
    try {
      await this.#deleteNode(nodeId, [...gone], title);
    } catch (error) {
      if (!isBackendError(error) || error.status !== 404) {
        this.#host.setNodes(before);
        for (const [id, mark] of marks) {
          if (mark) this.#forgottenAt.set(id, mark);
          else this.#forgottenAt.delete(id);
        }
        throw error;
      }
    }
    const shown = this.#host.openNode();
    if (shown && gone.has(shown.id)) shown.gone('reader', title);
  }

  async duplicate(nodeId: string, title: string, stepName: string): Promise<TDetail | undefined> {
    const dup = this.#host.duplicate;
    if (!dup) return undefined;
    let copied: TDetail | undefined;
    try {
      return await this.#host.workspace.undo.perform(
        async () => {
          if (copied) {
            await unlessGone(() => this.restore(copied?.id ?? ''));
            return copied;
          }
          copied = await dup(nodeId, title);
          this.#host.takeNode(copied);
          this.#host.reread();
          return copied;
        },
        (copy) => unlessGone(() => this.#deleteShown(copy.id)),
        stepName,
      );
    } catch (error) {
      this.#host.workspace.report('save', error);
      return undefined;
    }
  }

  async restore(nodeId: string): Promise<void> {
    this.#ownDeleted.delete(nodeId);
    const back = await this.#host.restore(nodeId);
    this.#host.takeNode(back);
    this.#host.reread();
  }

  async changeTitle(nodeId: string, title: string, stepName: string): Promise<void> {
    const patchTitle = this.#host.patchTitle;
    const before = this.#node(nodeId);
    if (!patchTitle || !before || before.title === title) return;
    const previous = before.title;
    const write = async (next: string, back: string): Promise<void> => {
      const show = (value: string): void => {
        this.#host.setNodes(
          this.#host.nodes().map((node) => (node.id === nodeId ? { ...node, title: value } : node)),
        );
      };
      show(next);
      try {
        const updated = await patchTitle(nodeId, next);
        if (this.receive(updated)) this.#host.takeNode(updated);
      } catch (error) {
        show(back);
        throw error;
      }
    };
    try {
      await this.#host.workspace.undo.perform(
        () => unlessGone(() => write(title, previous)),
        () => unlessGone(() => write(previous, title)),
        stepName,
      );
    } catch (error) {
      this.#host.workspace.report('save', error);
    }
  }

  move(nodeId: string, to: PagePlace, stepName: 'movePage' | 'moveUnit'): Promise<boolean> {
    const turn =
      this.#queued === 0
        ? this.#moveNode(nodeId, to, stepName)
        : this.#queue.then(() => this.#moveNode(nodeId, to, stepName));
    this.#queued += 1;
    this.#queue = turn.finally(() => {
      this.#queued -= 1;
    });
    return turn;
  }

  async #moveNode(
    nodeId: string,
    to: PagePlace,
    stepName: 'movePage' | 'moveUnit',
  ): Promise<boolean> {
    if (!movesAnywhere(this.#host.nodes(), nodeId, to)) return true;
    const back = placeOf(this.#host.nodes(), nodeId);
    if (!back) return false;
    let first = true;
    try {
      await this.#host.workspace.undo.perform(
        () => {
          if (!first) return unlessGone(() => this.#applyMove(nodeId, to));
          first = false;
          return this.#applyMove(nodeId, to);
        },
        () => unlessGone(() => this.#moveBack(nodeId, back)),
        stepName,
      );
      return true;
    } catch (error) {
      if (isBackendError(error) && error.status === 404) {
        this.forget([nodeId], 'others');
        this.#host.workspace.report(stepName, error);
      } else this.#host.workspace.report('save', error);
      return false;
    }
  }

  receive(node: TNode): boolean {
    return this.#pending.receive(node);
  }

  async #applyMove(nodeId: string, to: PagePlace): Promise<void> {
    const shown = this.#node(nodeId);
    if (shown && !this.#moving.has(nodeId)) this.#unmoved.set(nodeId, shown);
    this.#moving.set(nodeId, (this.#moving.get(nodeId) ?? 0) + 1);
    this.#pending.begin(nodeId);
    this.#host.setNodes(moved(this.#host.nodes(), nodeId, to));
    let answer: TNode | undefined;
    try {
      answer = await this.#host.move(nodeId, to);
    } finally {
      const latest = this.#pending.end(nodeId, answer);
      const left = (this.#moving.get(nodeId) ?? 1) - 1;
      if (left > 0) this.#moving.set(nodeId, left);
      else {
        this.#moving.delete(nodeId);
        const unmoved = this.#unmoved.get(nodeId);
        this.#unmoved.delete(nodeId);
        const show = latest ?? unmoved;
        if (show) this.#host.takeNode(show);
        if (answer && !isPlaced(this.#host.nodes(), nodeId, to)) this.#host.reread();
      }
    }
  }

  async #moveBack(nodeId: string, back: PagePlace): Promise<void> {
    try {
      await this.#applyMove(nodeId, back);
    } catch (error) {
      if (!isBackendError(error) || error.status !== 400) throw error;
      const parentRefused = error.refusalAt('/parentId') !== undefined;
      if (parentRefused && error.reason === 'page_too_deep') throw error;
      await this.#applyMove(nodeId, { parentId: parentRefused ? '' : back.parentId });
    }
  }
}
