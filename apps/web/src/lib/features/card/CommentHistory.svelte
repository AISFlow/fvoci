<!--
  What a comment said before it was changed, the text it was posted with
  first: who wrote each text and when, and what each changed from the one
  before it, words added and removed (compare-bodies.ts), as the versions of
  a body show it, but always from the text before and without folding (a
  comment is short). The first is shown as it is. Last, the text the comment
  has now, compared with the one before it: what the last change did. Only
  to read; nothing is put back from here. Read again when the comment is
  changed while it is open.
-->
<script lang="ts">
  import type { BodyReferences } from '$lib/editor/body-editor.ts';
  import BodyView from '$lib/editor/BodyView.svelte';
  import { comparable, compareBodies } from '$lib/editor/compare-bodies.ts';
  import DiffBlocks from '$lib/editor/DiffBlocks.svelte';
  import { diffCounts } from '$lib/editor/render-diff.ts';
  import type { DocNode } from '$lib/editor/schema.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { BodyDiff } from '$lib/logic/body-diff.ts';
  import { displayDateTime, type DisplayContext } from '$lib/logic/format.ts';
  import type { CommentVersion } from '$lib/state/index.ts';

  interface Props {
    /** Reads what a comment said before, with its writers known by name. */
    read: (commentId: string) => Promise<CommentVersion[]>;
    /** What the references in the texts are drawn as. */
    references: BodyReferences;
    commentId: string;
    /** When the comment was last changed: a new change reads the texts again. */
    editedAt: number;
    /** The text the comment has now, and who changed it to that. */
    body: DocNode | null;
    editedBy: string;
    context: DisplayContext;
    resolveUser: (userId: string) => string;
    id: string;
  }

  let { read, references, commentId, editedAt, body, editedBy, context, resolveUser, id }: Props =
    $props();

  /** A text of the comment, and what it changed from the one before (none for the first). */
  interface Text extends CommentVersion {
    now: boolean;
    /** Undefined when it is not compared: either body cannot be shown, or the comparison failed. */
    change: Promise<BodyDiff | undefined> | undefined;
  }

  /** The texts as of the comment's last change: a newer one reads them again. */
  const texts = $derived(textsAsOf(commentId, editedAt));

  async function textsAsOf(comment: string, changedAt: number): Promise<Text[]> {
    // Read before the wait, so that the texts are read again when the text now is another.
    const now = { version: Infinity, body, writtenBy: editedBy, writtenAt: changedAt, now: true };
    const earlier = await read(comment);
    const all = [...earlier.map((text) => ({ ...text, now: false })), now];
    return all.map((text, index) => ({
      ...text,
      change: changeOf(all[index - 1]?.body, text.body),
    }));
  }

  /** What `after` changed from `before`, both shown: none to compare for the first text. */
  function changeOf(
    before: DocNode | null | undefined,
    after: DocNode | null,
  ): Promise<BodyDiff | undefined> | undefined {
    if (before === undefined || !comparable(before) || !comparable(after)) return undefined;
    return compareBodies(before, after).catch(() => undefined);
  }

  function said(text: Text): string {
    const who = resolveUser(text.writtenBy);
    const when = displayDateTime(text.writtenAt, context);
    if (text.now) return m.card_comment_version_now({ who, when });
    return text.version === 1
      ? m.card_comment_version_posted({ who, when })
      : m.card_comment_version_changed({ who, when });
  }
</script>

{#snippet asItIs(text: Text)}
  <BodyView doc={text.body} {resolveUser} {references} class="text-sm">
    {#snippet invalid()}
      <p class="text-muted-foreground">{m.card_body_invalid()}</p>
    {/snippet}
  </BodyView>
{/snippet}

<!-- A tint, not a colour: a table of an older version, which nobody types in, has a clear frame on it. -->
<div
  {id}
  class="mt-2 rounded-lg border bg-muted/40 px-3 py-2 [--doc-surface:transparent]"
  data-testid="comment-history"
>
  {#await texts}
    <p class="text-sm text-muted-foreground">{m.app_loading()}</p>
  {:then list}
    <ol class="flex flex-col gap-3">
      {#each list as text (text.version)}
        <li data-version={text.now ? 'now' : text.version}>
          <p class="text-xs text-muted-foreground">{said(text)}</p>
          {#if text.change}
            {#await text.change}
              {@render asItIs(text)}
            {:then diff}
              {#if diff}
                <p class="text-xs text-muted-foreground" data-testid="comment-diff-counts">
                  {diff.changed ? diffCounts(diff) : m.body_diff_none()}
                </p>
                <DiffBlocks blocks={diff.blocks} {resolveUser} {references} class="text-sm" />
              {:else}
                {@render asItIs(text)}
              {/if}
            {/await}
          {:else}
            {@render asItIs(text)}
          {/if}
        </li>
      {/each}
    </ol>
  {:catch}
    <p class="text-sm text-muted-foreground" role="status">
      {m.card_comment_history_failed()}
    </p>
  {/await}
</div>
