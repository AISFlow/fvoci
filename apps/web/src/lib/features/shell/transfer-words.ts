/**
 * The words of the stack of transfers (Transfers.svelte): what each says it
 * is doing, what it did, and why an upload failed.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { humanFileSize } from '$lib/logic/format.ts';
import type { Transfer, UploadFailure } from '$lib/state/index.ts';

const nameOf = (transfer: Transfer): string => transfer.name || m['Sidebar.untitled-board']();

/** What the one card of many uploads says: how many are going up, failed, or are up. */
export function uploadsLabel(running: number, failed: number, all: number): string {
  if (running > 0) {
    return running === 1
      ? m.shell_uploads_running_one()
      : m.shell_uploads_running({ count: running });
  }
  if (failed > 0) return m.shell_uploads_failed({ count: failed });
  return m.shell_uploads_done({ count: all });
}

/** What a transfer is doing, or did: its line in the stack. */
export function transferLabel(transfer: Transfer): string {
  const name = nameOf(transfer);
  if (transfer.failure === 'kept') return m.shell_transfer_kept_upload({ name });
  if (transfer.failure) return m.shell_transfer_failed_upload({ name });
  if (transfer.finished) {
    if (transfer.kind === 'upload') return m.shell_transfer_done_upload({ name });
    return transfer.kind === 'export'
      ? m.shell_transfer_done_export({ name })
      : m.shell_transfer_done_import({ name });
  }
  if (transfer.kind === 'upload') return m.shell_transfer_upload({ name });
  return transfer.kind === 'export'
    ? m.shell_transfer_export({ name })
    : m.shell_transfer_import({ name });
}

const FAILURE: Record<Exclude<UploadFailure, 'too-large'>, () => string> = {
  refused: m.shell_upload_failure_refused,
  network: m.shell_upload_failure_network,
  forbidden: m.shell_upload_failure_forbidden,
  busy: m.shell_upload_failure_busy,
  room: m.shell_upload_failure_room,
  left: m.shell_upload_failure_left,
  kept: m.shell_upload_failure_kept,
  failed: m.shell_upload_failure_failed,
};

/** Why an upload failed; empty for one that did not. */
export function failureText(transfer: Transfer): string {
  const { failure, limit } = transfer;
  if (!failure) return '';
  if (failure !== 'too-large') return FAILURE[failure]();
  return limit
    ? m.shell_upload_failure_too_large({ limit: humanFileSize(limit) })
    : m.shell_upload_failure_too_large_unknown();
}

/** What a screen reader is told of a start, an end or a failure. */
export function transferSaying(transfer: Transfer): string {
  return [transferLabel(transfer), failureText(transfer)].filter(Boolean).join(' ');
}

/** How much has gone, of how much, while under way; empty before anything has. */
export function progressText(transfer: Transfer): string {
  if (transfer.done <= 0) return '';
  return transfer.total
    ? m.shell_transfer_progress({
        done: humanFileSize(transfer.done),
        total: humanFileSize(transfer.total),
      })
    : humanFileSize(transfer.done);
}

/** Under way: neither finished nor failed. */
export const underWay = (transfer: Transfer): boolean => !transfer.finished && !transfer.failure;
