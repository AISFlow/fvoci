<script lang="ts" module>
  /** What the stack holds: one on its way, one in, ones that failed, or many at once. */
  export type TransfersShown = 'progress' | 'done' | 'failed' | 'many';
</script>

<script lang="ts">
  import { onMount } from 'svelte';

  import Transfers from '$lib/features/shell/Transfers.svelte';
  import { BackendError, type TransferOptions } from '$lib/state/index.ts';

  import { exampleWorkspace, loadWorkspace } from './workspace.ts';

  let { shown = 'progress' }: { shown?: TransfersShown } = $props();

  const MIB = 1024 * 1024;
  const example = exampleWorkspace({ count: 0 });
  const file = (name: string, size: number): File =>
    // A file of that size, with no bytes to hold: only its length is read here.
    Object.defineProperty(new File([], name), 'size', { value: size });

  /** Goes `sent` bytes of 17 MiB, and no further. */
  const partWay =
    (sent: number) =>
    ({ onProgress }: TransferOptions): Promise<never> => {
      onProgress({ sentBytes: sent, totalBytes: 17 * MIB });
      return new Promise(() => undefined);
    };

  function start(): void {
    const { transfers } = example.workspace;
    const upload = (name: string, size: number, send: (o: TransferOptions) => Promise<unknown>) =>
      transfers.upload({ file: file(name, size), send, retryable: true });
    switch (shown) {
      case 'progress':
        upload(
          '회의록 2026-11 · Minutes of the long example meeting.hwp',
          17 * MIB,
          partWay(8 * MIB),
        );
        break;
      case 'done':
        upload('scan.pdf', 2 * MIB, () => Promise.resolve({ fileId: 'example-file' }));
        break;
      case 'failed':
        upload('video.mp4', 140 * MIB, () =>
          Promise.reject(new BackendError(413, { limit: 100 * MIB, reason: 'file_too_large' })),
        );
        upload('notes.txt', 3 * 1024, () => Promise.reject(new BackendError(0, {})));
        break;
      case 'many':
        for (let index = 1; index <= 6; index += 1) {
          upload(`photo-${String(index)}.png`, 17 * MIB, partWay(index * 2 * MIB));
        }
        break;
    }
  }

  onMount(() => {
    void loadWorkspace(example).then(start);
    return () => {
      example.workspace.close();
    };
  });
</script>

<div
  class="min-h-96"
  data-testid="transfers-example"
  data-fixture-ready={example.workspace.status === 'ready' ? '' : undefined}
>
  <Transfers workspace={example.workspace} />
</div>
