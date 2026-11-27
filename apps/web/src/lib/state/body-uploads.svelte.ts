/**
 * The files going up for a body, a card's description or a wiki page's. Each
 * is shown where every upload is (`Transfers`), and its place in the body
 * by the editor.
 *
 * Each is given up when what it was going up for closes: its block would be
 * put in nowhere. It stays in the stack then, failed for that reason, until
 * the reader dismisses it, wherever they went.
 */
import type { UploadedFile } from './backend.ts';
import type { TransferOptions, Transfers } from './transfers.svelte.ts';

export class BodyUploads {
  /** The transfers of those under way. */
  #underWay = $state.raw<number[]>([]);
  readonly #place: () => { readonly transfers: Transfers };

  /** `place` (the workspace) is asked for its stack when an upload starts. */
  constructor(place: () => { readonly transfers: Transfers }) {
    this.#place = place;
  }

  /** A file is going up: closing now would give it up. */
  get goingUp(): boolean {
    return this.#underWay.length > 0;
  }

  /**
   * Uploads `file` with `send`, and gives the identifier of the file. It
   * rejects when the upload failed (the stack says why) or was given up.
   */
  async run(
    file: File,
    send: (options: TransferOptions) => Promise<UploadedFile>,
  ): Promise<string> {
    const { id, done } = this.#place().transfers.upload({ file, send });
    this.#underWay = [...this.#underWay, id];
    try {
      const uploaded = await done;
      if (!uploaded) throw new Error(`${file.name} did not go up`);
      return uploaded.fileId;
    } finally {
      this.#underWay = this.#underWay.filter((other) => other !== id);
    }
  }

  /** What they were going up for closes: those under way are given up, and stay told so. */
  giveUp(): void {
    const underWay = this.#underWay;
    this.#underWay = [];
    const { transfers } = this.#place();
    for (const id of underWay) transfers.leave(id);
  }
}
