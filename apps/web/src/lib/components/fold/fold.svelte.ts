/**
 * Whether a part of a panel that folds shows what it holds: as it stands by
 * itself (`byItself`, asked each time, so that it may follow what was read),
 * until the reader folds or unfolds it, which then holds. Once it has stood
 * open by itself it stays open: the last of what it holds going (a group
 * taken off, a link withdrawn) does not fold it away under the reader, with
 * the button they pressed and the focus on it.
 */
export class Fold {
  #byHand = $state<boolean | undefined>(undefined);
  readonly #byItself: () => boolean;
  /** It stood open by itself once. Not state: it is learnt while the fold is drawn. */
  #wasOpen = false;

  constructor(byItself: () => boolean = () => false) {
    this.#byItself = byItself;
  }

  get open(): boolean {
    if (this.#byHand !== undefined) return this.#byHand;
    if (this.#wasOpen) return true;
    this.#wasOpen = this.#byItself();
    return this.#wasOpen;
  }

  toggle(): void {
    this.#byHand = !this.open;
  }
}
