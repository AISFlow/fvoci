/**
 * Account-wide and single-workspace `'Today'` hub queries (`memoryAccountToday`,
 * `memoryWorkspaceToday`) for `MemoryBackend` (`state/memory/backend.ts`).
 */
import { dateOfInstant } from '../../logic/dates.ts';
import { cardProperty, parseDateValue, type Card, type Container } from '../../logic/wire.ts';
import type {
  AccountToday,
  TodayCardItem,
  TodayMeetingItem,
  TodayReviewEntry,
  TodayUnitEntry,
  TodayWorkspaceMeta,
  WorkspaceToday,
} from '../backend.ts';

import { settingsOf } from './account.ts';
import type { MemoryContainerContext } from './container-ops.ts';
import { memoryGetWorkspaceLearningToday } from './course-schedules.ts';
import { summaryOf } from './shapes.ts';
import * as spaces from './spaces.ts';
import { MemoryUnitReviews } from './unit-reviews.ts';
import type { StoredCard, World } from './world.ts';

function isCardDone(board: Container, card: Card): boolean {
  const rule = board.doneWhen;
  if (!rule) return false;
  const val = cardProperty(card, rule.propertyId);
  if (rule.kind === 'checkbox') return val === 'true';
  const ids = Array.isArray(val) ? val : typeof val === 'string' ? [val] : [];
  return ids.some((id) => typeof id === 'string' && rule.optionIds.includes(id));
}

function workspaceMetaOf(world: World, userId: string, workspaceId: string): TodayWorkspaceMeta {
  const ws = spaces.workspacesOf(world, userId).find((w) => w.id === workspaceId);
  return {
    workspaceId,
    workspaceName: ws?.title ?? world.workspace.title,
    orgId: ws?.orgId ?? 'org-memory',
    orgName: ws?.title ?? world.workspace.title,
    isPersonal: ws?.kind === 'personal',
  };
}

function memoryTodayCards(
  ctx: MemoryContainerContext,
  workspaceId: string,
  meta: TodayWorkspaceMeta,
  today: string,
  soonCutoff: string,
  timeZone: string,
): TodayCardItem[] {
  const cards: TodayCardItem[] = [];
  for (const board of ctx.world.containers.values()) {
    if (
      board.workspaceId !== workspaceId ||
      (board.kind ?? 'board') !== 'board' ||
      board.isTemplate
    )
      continue;
    const duePropId =
      board.dueDatePropertyId ?? board.cardProperties.find((p) => p.type === 'date')?.id;
    if (!duePropId) continue;
    const personProps = board.cardProperties.filter(
      (p) => p.type === 'person' || p.type === 'multiPerson',
    );
    for (const raw of ctx.world.blocksOf(board.id)) {
      if (raw.type !== 'card') continue;
      const card = summaryOf(raw as StoredCard);
      if (card.fields.isTemplate || isCardDone(board, card)) continue;
      const assigned = personProps.some((p) => {
        const v = cardProperty(card, p.id);
        return (
          v === ctx.userId || (Array.isArray(v) && (v as readonly unknown[]).includes(ctx.userId))
        );
      });
      if (!assigned) continue;
      const parsed = parseDateValue(cardProperty(card, duePropId));
      const point = parsed.to ?? parsed.from;
      if (!point) continue;
      const dueDate = point.date ?? dateOfInstant(point.instant, timeZone);
      if (dueDate > soonCutoff) continue;
      const overdueDays =
        dueDate < today
          ? Math.round(
              (Date.parse(`${today}T00:00:00Z`) - Date.parse(`${dueDate}T00:00:00Z`)) / 86_400_000,
            )
          : 0;
      cards.push({
        workspace: meta,
        boardId: board.id,
        boardTitle: board.title,
        boardIcon: board.icon ?? '',
        cardId: card.id,
        cardNumber: card.number ?? null,
        title: card.title,
        icon: card.fields.icon ?? '',
        dueDate,
        dueAtMillis: point.instant ?? null,
        overdueDays,
      });
    }
  }
  return cards.sort((a, b) => a.dueDate.localeCompare(b.dueDate));
}

function memoryTodayMeetings(
  ctx: MemoryContainerContext,
  workspaceId: string,
  meta: TodayWorkspaceMeta,
  now: number,
): TodayMeetingItem[] {
  const meetings: TodayMeetingItem[] = [];
  for (const course of ctx.world.containers.values()) {
    if (course.workspaceId !== workspaceId || course.kind !== 'course' || course.isTemplate)
      continue;
    if (!(ctx.world.members.get(course.id) ?? []).some((m) => m.userId === ctx.userId)) continue;
    for (const meeting of ctx.world.courseMeetings.get(course.id) ?? []) {
      if (meeting.startsAt >= now - 3_600_000 && meeting.startsAt <= now + 86_400_000) {
        meetings.push({
          workspace: meta,
          containerId: course.id,
          courseTitle: course.title,
          meetingId: meeting.id,
          title: meeting.title,
          startsAt: meeting.startsAt,
          endsAt: null,
          location: meeting.location,
        });
      }
    }
  }
  return meetings.sort((a, b) => a.startsAt - b.startsAt);
}

export async function memoryWorkspaceToday(
  ctx: MemoryContainerContext,
  workspaceId: string,
): Promise<WorkspaceToday> {
  spaces.checkWorkspace(ctx.world, ctx.userId, workspaceId);
  const rawTz = settingsOf(ctx.world, ctx.userId).timeZone;
  const timeZone = !rawTz || rawTz === 'auto' ? 'UTC' : rawTz;
  const now = ctx.world.now();
  const today = dateOfInstant(now, timeZone);
  const soonCutoff = dateOfInstant(now + 2 * 86_400_000, timeZone);
  const meta = workspaceMetaOf(ctx.world, ctx.userId, workspaceId);
  const cards = memoryTodayCards(ctx, workspaceId, meta, today, soonCutoff, timeZone);
  const learning = memoryGetWorkspaceLearningToday(ctx.world, ctx.userId, workspaceId);
  const units: TodayUnitEntry[] = [...learning.overdueUnits, ...learning.todayUnits].map((u) => ({
    ...u,
    workspace: meta,
  }));
  const unitReviews = new MemoryUnitReviews(ctx, () => Promise.resolve());
  const reviews: TodayReviewEntry[] = (await unitReviews.getWorkspaceTodayReviews(workspaceId)).map(
    (r) => ({ ...r, workspace: meta }),
  );
  const meetings = memoryTodayMeetings(ctx, workspaceId, meta, now);
  return {
    today,
    workspace: meta,
    cards,
    hasMoreCards: false,
    units,
    hasMoreUnits: false,
    reviews,
    hasMoreReviews: false,
    meetings,
    hasMoreMeetings: false,
  };
}

export async function memoryAccountToday(ctx: MemoryContainerContext): Promise<AccountToday> {
  const rawTz = settingsOf(ctx.world, ctx.userId).timeZone;
  const timeZone = !rawTz || rawTz === 'auto' ? 'UTC' : rawTz;
  const today = dateOfInstant(ctx.world.now(), timeZone);
  const all = spaces.workspacesOf(ctx.world, ctx.userId);
  const open = all.filter((w) => !w.ssoRequired && !w.passwordRequired);
  const lockedWorkspaceCount = all.length - open.length;
  const perWs = await Promise.all(open.map((w) => memoryWorkspaceToday(ctx, w.id)));
  return {
    today,
    lockedWorkspaceCount,
    cards: perWs.flatMap((w) => w.cards).sort((a, b) => a.dueDate.localeCompare(b.dueDate)),
    hasMoreCards: false,
    units: perWs.flatMap((w) => w.units),
    hasMoreUnits: false,
    reviews: perWs.flatMap((w) => w.reviews),
    hasMoreReviews: false,
    meetings: perWs.flatMap((w) => w.meetings).sort((a, b) => a.startsAt - b.startsAt),
    hasMoreMeetings: false,
  };
}
