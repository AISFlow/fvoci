/**
 * Reactions: the backend in memory keeps the server's rules
 * (`crates/server/src/api/reactions.rs`), and the page shown keeps them
 * apart from its comments (`CommentThreadStore`): the reader's toggle shows
 * at once and goes back when refused, and what somebody else does comes as
 * the block's reactions as they are now, which a read asked before does not
 * put older ones over.
 */
import { describe, expect, it, vi } from 'vitest';

import type { MemberRole } from '$lib/logic/wire.ts';
import { BackendError, WikiStore, type Change } from '$lib/state/index.ts';
import { emojiRefusal } from '$lib/state/memory/emoji.ts';
import { eraseUser } from '$lib/state/memory/users.ts';
import { withMine, type Reaction } from '$lib/state/reaction-types.ts';

import table from '../../src/lib/editor/emoji/en.json';

import { openFirstBoard, settle, setup, text } from './support.ts';

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('the call did not reject');
};

/** A wiki of the reader with a page and a comment on it, the other person on it with `role`. */
async function aPage(role: Exclude<MemberRole, '' | 'admin'> = 'commenter') {
  const made = setup();
  const workspaceId = made.world.workspace.id;
  const wiki = await made.backend.wiki.createWiki(workspaceId, {
    title: 'Handbook',
    visibility: 'open',
  });
  await made.backend.patchContainer(wiki.id, { minimumRole: role });
  await made.other.joinContainer(wiki.id);
  const page = await made.backend.wiki.createPage(wiki.id, { title: 'Page' });
  const comment = await made.backend.wiki.createComment(wiki.id, page.id, text('hello'));
  return { ...made, workspaceId, wikiId: wiki.id, pageId: page.id, commentId: comment.id };
}

describe('the reactions of a reader', () => {
  const [ann, bob] = ['ann', 'bob'];

  it('go on last and come off, the reader last among those who reacted', () => {
    const none: Reaction[] = [];
    const one = withMine(none, '👍', ann, true);
    expect(one).toEqual([{ emoji: '👍', userIds: [ann] }]);
    const two = withMine(withMine(one, '🎉', bob, true), '👍', bob, true);
    expect(two).toEqual([
      { emoji: '👍', userIds: [ann, bob] },
      { emoji: '🎉', userIds: [bob] },
    ]);
    expect(withMine(two, '👍', ann, true)).toBe(two);
    expect(withMine(withMine(two, '🎉', bob, false), '👍', ann, false)).toEqual([
      { emoji: '👍', userIds: [bob] },
    ]);
    expect(withMine(none, '👀', ann, false)).toBe(none);
  });
});

describe('an emoji, as the server takes it', () => {
  it('is any the picker offers, and each shape of one', () => {
    const offered = table.emoji.split(' ');
    expect(offered.length).toBeGreaterThan(1000);
    expect(offered.filter((one) => emojiRefusal(one) !== undefined)).toEqual([]);
    const shapes = [
      '👍🏽',
      '👩🏽‍💻',
      '🏳️‍🌈',
      '❤️‍🔥',
      '👁️‍🗨️',
      '👨‍👩‍👧‍👦',
      '🇰🇷',
      '🏴󠁧󠁢󠁳󠁣󠁴󠁿',
      '#️⃣',
      '1️⃣',
      '©️',
      '™',
      '🀄',
      '🏃‍➡️',
      '🏽',
      '➕',
      '➗',
      '➡️',
      '➰',
    ];
    expect(shapes.filter((one) => emojiRefusal(one) !== undefined)).toEqual([]);
  });

  it('is not a letter, two of them, a part alone, the text look, nor a dingbat digit', () => {
    const none = [
      'a',
      '1',
      '#',
      ' ',
      '가',
      'ok',
      '👍👍',
      '👍 ',
      '🇰🇷🇯🇵',
      '\u200D',
      '👍\u200D',
      '\u200D👍',
      '\uFE0F',
      '🇰',
      '👍\uFE0E',
      '🏴\u{E0067}\u{E0062}',
      '🏴\u{E0041}\u{E007F}',
      '👍\u0301',
      '❶',
      '➓',
      '➔',
    ];
    expect(none.filter((one) => emojiRefusal(one) !== 'a reaction is one emoji')).toEqual([]);
    expect(emojiRefusal('')).toBe('an emoji is not empty');
    expect(emojiRefusal('👩🏻‍❤️‍💋‍👨🏼')).toBe('an emoji is at most 8 characters');
  });
});

describe('reactions in memory', () => {
  it('are added once by who may comment, read with the comments and the page, and told', async () => {
    const made = await aPage('commenter');
    const told: Change[] = [];
    made.backend.watchContainer(made.wikiId, (change) => told.push(change));
    const { reactions } = made.other;
    const at = [made.workspaceId, made.wikiId] as const;
    await reactions.add(...at, made.commentId, '👍');
    await reactions.add(...at, made.commentId, '👍');
    await reactions.add(...at, made.pageId, '🎉');
    const [comment] = await made.backend.wiki.comments(made.wikiId, made.pageId);
    expect(comment?.reactions).toEqual([{ emoji: '👍', userIds: [made.other.userId] }]);
    const page = await made.backend.wiki.page(made.wikiId, made.pageId);
    expect(page.reactions).toEqual([{ emoji: '🎉', userIds: [made.other.userId] }]);
    await reactions.remove(...at, made.commentId, '👍');
    await reactions.remove(...at, made.commentId, '👍');
    const [again] = await made.backend.wiki.comments(made.wikiId, made.pageId);
    expect(again?.reactions).toBeUndefined();
    await settle();
    expect(told.map((change) => change.kind)).toEqual(['reactions', 'reactions', 'reactions']);
  });

  it('are refused when the emoji is none, before anything else, adding and taking off', async () => {
    const made = await aPage('viewer');
    const at = [made.workspaceId, made.wikiId, 'nothing', '❶'] as const;
    const theirs = made.other.reactions;
    for (const call of [() => theirs.add(...at), () => theirs.remove(...at)]) {
      const refused = await rejection(call());
      expect([refused.status, refused.errors[0]?.parameter]).toEqual([400, 'emoji']);
    }
  });

  it('of an account erased go with it, those of a page in the trash too', async () => {
    const made = await aPage();
    const at = [made.workspaceId, made.wikiId] as const;
    const [mine, theirs] = [made.backend.reactions, made.other.reactions];
    await theirs.add(...at, made.commentId, '👍');
    await mine.add(...at, made.commentId, '👍');
    await theirs.add(...at, made.pageId, '🎉');
    await made.backend.wiki.deletePage(made.wikiId, made.pageId);
    eraseUser(made.world, made.other.userId);
    expect(made.world.reactions.get(made.commentId)).toEqual([
      { emoji: '👍', userIds: [made.backend.userId] },
    ]);
    expect(made.world.reactions.has(made.pageId)).toBe(false);
  });

  it('are refused to a viewer, past twenty emoji, and on what is not there', async () => {
    const made = await aPage('viewer');
    const at = [made.workspaceId, made.wikiId] as const;
    const theirs = made.other.reactions;
    const mine = made.backend.reactions;
    expect((await rejection(theirs.add(...at, made.commentId, '👍'))).status).toBe(403);
    // Who may not comment is told so before the block is looked for, as by the server.
    expect((await rejection(theirs.add(...at, 'nothing', '👍'))).status).toBe(403);
    const animals = Array.from({ length: 21 }, (_, at) => String.fromCodePoint(0x1f400 + at));
    for (const animal of animals.slice(0, 20)) await mine.add(...at, made.pageId, animal);
    const refused = await rejection(mine.add(...at, made.pageId, animals[20] ?? ''));
    expect([refused.status, refused.reason]).toEqual([400, 'too_many_reactions']);
    expect((await rejection(mine.add(...at, 'nothing', '👍'))).status).toBe(404);
  });
});

describe('the reactions of the page shown', () => {
  it('show the reader’s toggle at once, keep it, and take it back when refused', async () => {
    const made = await aPage();
    await made.workspace.load();
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    const comments = store.showPage(made.pageId)?.comments;
    await settle();
    if (!comments) throw new Error('no page shown');
    expect(comments.canReact).toBe(true);
    const me = made.backend.userId;

    made.backend.latency = 20;
    const adding = comments.toggleReaction(made.commentId, '🚀');
    expect(comments.reactionsOf(made.commentId)).toEqual([{ emoji: '🚀', userIds: [me] }]);
    await adding;
    await settle();
    expect(comments.reactionsOf(made.commentId)).toEqual([{ emoji: '🚀', userIds: [me] }]);

    made.backend.latency = 0;
    made.backend.failNext('reactions', new BackendError(500, { detail: 'down' }));
    const refused = comments.toggleReaction(made.commentId, '🚀');
    expect(comments.reactionsOf(made.commentId)).toEqual([]);
    await refused;
    expect(comments.reactionsOf(made.commentId)).toEqual([{ emoji: '🚀', userIds: [me] }]);
    store.close();
  });

  it('follow what somebody else does, on the page and on its comments', async () => {
    const made = await aPage();
    await made.workspace.load();
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    const comments = store.showPage(made.pageId)?.comments;
    await settle();
    if (!comments) throw new Error('no page shown');
    const theirs = made.other.reactions;
    const at = [made.workspaceId, made.wikiId] as const;
    await theirs.add(...at, made.pageId, '👀');
    await theirs.add(...at, made.commentId, '❤️');
    await settle();
    expect(comments.reactionsOf(made.pageId)).toEqual([
      { emoji: '👀', userIds: [made.other.userId] },
    ]);
    expect(comments.reactionsOf(made.commentId)).toEqual([
      { emoji: '❤️', userIds: [made.other.userId] },
    ]);
    // A comment changed carries none, and keeps its own.
    const [comment] = await made.backend.wiki.comments(made.wikiId, made.pageId);
    if (!comment) throw new Error('no comment');
    await made.backend.wiki.patchComment(made.wikiId, made.pageId, comment.id, {
      body: text('hello again'),
      readAt: comment.updatedAt,
    });
    await settle();
    expect(comments.reactionsOf(made.commentId)).toHaveLength(1);
    await theirs.remove(...at, made.commentId, '❤️');
    await settle();
    expect(comments.reactionsOf(made.commentId)).toEqual([]);
    store.close();
  });

  it('are read on a page opened later', async () => {
    const made = await aPage();
    const theirs = made.other.reactions;
    await theirs.add(made.workspaceId, made.wikiId, made.pageId, '😄');
    await theirs.add(made.workspaceId, made.wikiId, made.commentId, '👍');
    await made.workspace.load();
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    const comments = store.showPage(made.pageId)?.comments;
    await settle();
    if (!comments) throw new Error('no page shown');
    expect(comments.reactionsOf(made.pageId).map((one) => one.emoji)).toEqual(['😄']);
    expect(comments.reactionsOf(made.commentId).map((one) => one.emoji)).toEqual(['👍']);
    expect(comments.comments[0]).not.toHaveProperty('reactions');
    store.close();
  });
});

/** A promise held until it is let go, and what lets it go. */
function held() {
  let release = (): void => undefined;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

/** Makes the next call of `name` on `on` answer as it would now, but only once `gate` is let go. */
function answeredLate<T extends object>(on: T, name: keyof T & string, gate: Promise<void>): void {
  const real = (on[name] as (...args: unknown[]) => Promise<unknown>).bind(on);
  vi.spyOn(on, name as never).mockImplementationOnce((async (...args: unknown[]) => {
    const now = await real(...args);
    await gate;
    return now;
  }) as never);
}

describe('the reactions told while a read of them is under way', () => {
  it('are kept over those the read of the page and of its comments carried', async () => {
    const made = await aPage();
    await made.workspace.load();
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    const wikis = made.backend.wiki;
    const gate = held();
    answeredLate(wikis, 'page', gate.promise);
    answeredLate(wikis, 'comments', gate.promise);
    const comments = store.showPage(made.pageId)?.comments;
    if (!comments) throw new Error('no page shown');
    // Both reads have been answered as things were; then somebody else reacts.
    await settle();
    const theirs = made.other.reactions;
    const at = [made.workspaceId, made.wikiId] as const;
    await theirs.add(...at, made.pageId, '👀');
    await theirs.add(...at, made.commentId, '❤️');
    await settle();
    gate.release();
    await settle();
    expect(comments.status).toBe('ready');
    expect(comments.reactionsOf(made.pageId).map((one) => one.emoji)).toEqual(['👀']);
    expect(comments.reactionsOf(made.commentId).map((one) => one.emoji)).toEqual(['❤️']);
    store.close();
  });

  it('are kept over those the card read again carried, and a read asked after is taken', async () => {
    const { board, backend } = await openFirstBoard();
    const made = await backend.createCard(board.boardId, { title: 'Mine' });
    const card = await board.showCard(made.id);
    await settle();
    const gate = held();
    answeredLate(backend, 'card', gate.promise);
    answeredLate(backend, 'comments', gate.promise);
    const refreshing = card.refresh();
    await settle();
    const now = [{ emoji: '🎉', userIds: ['someone-elsewhere'] }];
    backend.world.reactions.set(made.id, now);
    card.receiveReactions(made.id, now);
    gate.release();
    await refreshing;
    expect(card.reactions.reactionsOf(made.id)).toEqual(now);

    // Taken off by a message missed; the next read is asked after, and is taken.
    backend.world.reactions.delete(made.id);
    await card.refresh();
    expect(card.reactions.reactionsOf(made.id)).toEqual([]);
  });
});

describe('the reactions told of a block the page does not show', () => {
  /** The page of `made` shown, its comments read once `gate` is let go. */
  async function shown(made: Awaited<ReturnType<typeof aPage>>, gate?: Promise<void>) {
    await made.workspace.load();
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    if (gate) answeredLate(made.backend.wiki, 'comments', gate);
    const comments = store.showPage(made.pageId)?.comments;
    if (!comments) throw new Error('no page shown');
    return { store, comments };
  }
  const told: Reaction[] = [{ emoji: '👍', userIds: ['someone'] }];

  it('are kept while the comments are read, and forgotten once they are, if no comment is it', async () => {
    const made = await aPage();
    const gate = held();
    const { store, comments } = await shown(made, gate.promise);
    comments.receiveReactions('elsewhere', told);
    comments.receiveReactions(made.commentId, told);
    expect(comments.reactionsOf('elsewhere')).toEqual(told);
    gate.release();
    await settle();
    expect(comments.status).toBe('ready');
    expect(comments.reactionsOf('elsewhere')).toEqual([]);
    expect(comments.reactionsOf(made.commentId)).toEqual(told);
    store.close();
  });

  it('are not kept once the comments failed to be read', async () => {
    const made = await aPage();
    vi.spyOn(made.backend.wiki, 'comments').mockRejectedValueOnce(new BackendError(500));
    vi.spyOn(made.workspace, 'report').mockImplementation(() => undefined);
    const { store, comments } = await shown(made);
    await settle();
    expect(comments.status).toBe('failed');
    comments.receiveReactions('elsewhere', told);
    expect(comments.reactionsOf('elsewhere')).toEqual([]);
    store.close();
  });

  it('by a toggle answered once the page is closed are not told', async () => {
    const made = await aPage();
    const { store, comments } = await shown(made);
    await settle();
    made.backend.latency = 20;
    const adding = comments.toggleReaction(made.commentId, '🚀');
    comments.close();
    await adding;
    expect(comments.reactionsOf(made.commentId)).toEqual([]);
    store.close();
  });
});

describe('the reactions of the card shown', () => {
  it('are read again with the card when the reader comes back, and ride on no card of the board', async () => {
    const { board, backend } = await openFirstBoard();
    const made = await backend.createCard(board.boardId, { title: 'Mine' });
    backend.world.reactions.set(made.id, [{ emoji: '🎉', userIds: ['someone-elsewhere'] }]);
    const card = await board.showCard(made.id);
    await settle();
    expect(card.reactions.reactionsOf(made.id).map((one) => one.emoji)).toEqual(['🎉']);
    expect(board.card(made.id)).not.toHaveProperty('reactions');

    // Added while the reader was away: nobody told them, and the card is as it was.
    const before = board.card(made.id)?.updatedAt;
    backend.world.reactions.set(made.id, [
      { emoji: '🎉', userIds: ['someone-elsewhere'] },
      { emoji: '👀', userIds: ['someone-elsewhere'] },
    ]);
    await card.refresh();
    expect(card.reactions.reactionsOf(made.id).map((one) => one.emoji)).toEqual(['🎉', '👀']);
    expect(board.card(made.id)?.updatedAt).toBe(before);
    expect(board.card(made.id)).not.toHaveProperty('reactions');
  });
});
