/**
 * Unit schedule field validation (`amount`, `amountUnit`, `scheduledDate`) and
 * unit restore logic for the in-memory course backend (`memory/courses.ts`).
 */
import { isAmountUnit, isValidUnitAmount } from '../../logic/course-schedule.ts';
import { parseCalendarDate, type CalendarDate } from '../../logic/dates.ts';
import { PAGE_TITLE_MAX } from '../../logic/page-tree.ts';
import {
  UNIT_AMOUNT_MAX,
  UNIT_AMOUNT_MIN,
  type CourseUnitDetail,
  type UnitDraft,
  type UnitKind,
  type UnitPatch,
} from '../course-types.ts';

import type { PageTree, Place } from './page-tree.ts';
import { fail } from './refuse.ts';
import { commentOf, copy, orderOf, unitOf } from './shapes.ts';
import type { StoredBlock, World } from './world.ts';

export const isCourseBlock = (block: StoredBlock | undefined): boolean =>
  block?.type === 'unit' || block?.type === 'chapter';

export function unitDetailOf(block: StoredBlock): CourseUnitDetail {
  return {
    ...unitOf(block),
    body: block.type === 'chapter' ? null : copy(block.body ?? null),
    createdBy: block.createdBy,
    modifiedBy: block.modifiedBy,
  };
}

export function checkedUnitTitle(title: string): string {
  if (Array.from(title).length > PAGE_TITLE_MAX) {
    fail(400, 'the title is too long', { field: 'title', limit: PAGE_TITLE_MAX });
  }
  return title;
}

function checkDateField(field: string, raw: string): CalendarDate {
  const parsed = parseCalendarDate(raw);
  if (!parsed) {
    fail(400, 'a date must be a valid calendar day written as YYYY-MM-DD', {
      field,
      reason: 'invalid_format',
    });
  }
  return parsed;
}

/** Validates and extracts optional schedule fields for a newly created unit or chapter. */
export function checkedUnitScheduleFields(
  kind: UnitKind,
  draft: Pick<UnitDraft, 'amount' | 'amountUnit' | 'scheduledDate'>,
): Record<string, unknown> {
  if (kind === 'chapter') {
    if (draft.amount !== undefined && draft.amount !== null) {
      fail(400, 'a chapter has no study amount', { field: 'amount', reason: 'invalid' });
    }
    if (draft.amountUnit !== undefined && draft.amountUnit !== null) {
      fail(400, 'a chapter has no amount unit', { field: 'amountUnit', reason: 'invalid' });
    }
    if (draft.scheduledDate !== undefined && draft.scheduledDate !== null) {
      fail(400, 'a chapter has no scheduled date', { field: 'scheduledDate', reason: 'invalid' });
    }
    return {};
  }
  const fields: Record<string, unknown> = {};
  if (draft.amount !== undefined && draft.amount !== null) {
    if (!isValidUnitAmount(draft.amount)) {
      fail(400, `amount must be an integer from ${UNIT_AMOUNT_MIN} to ${UNIT_AMOUNT_MAX}`, {
        field: 'amount',
        reason: 'invalid',
      });
    }
    fields.amount = draft.amount;
  }
  if (draft.amountUnit !== undefined && draft.amountUnit !== null) {
    if (!isAmountUnit(draft.amountUnit)) {
      fail(400, 'amountUnit must be one of minutes, pages, or problems', {
        field: 'amountUnit',
        reason: 'invalid',
      });
    }
    fields.amountUnit = draft.amountUnit;
  }
  if (draft.scheduledDate !== undefined && draft.scheduledDate !== null) {
    fields.scheduledDate = checkDateField('scheduledDate', draft.scheduledDate);
  }
  return fields;
}

/** Validates and applies tri-state schedule fields onto a unit block (`400` on chapters even for `null`). */
export function patchedUnitScheduleFields(
  node: StoredBlock,
  patch: Pick<UnitPatch, 'amount' | 'amountUnit' | 'scheduledDate'>,
): Record<string, unknown> | undefined {
  const hasAmount = 'amount' in patch && patch.amount !== undefined;
  const hasAmountUnit = 'amountUnit' in patch && patch.amountUnit !== undefined;
  const hasScheduled = 'scheduledDate' in patch && patch.scheduledDate !== undefined;
  if (!hasAmount && !hasAmountUnit && !hasScheduled) return undefined;
  if (node.type === 'chapter') {
    const field = hasAmount ? 'amount' : hasAmountUnit ? 'amountUnit' : 'scheduledDate';
    fail(400, `a chapter has no ${field}`, { field, reason: 'invalid' });
  }
  const next: Record<string, unknown> = { ...node.fields };
  if (patch.amount !== undefined) {
    if (patch.amount === null) delete next.amount;
    else {
      if (!isValidUnitAmount(patch.amount)) {
        fail(400, `amount must be an integer from ${UNIT_AMOUNT_MIN} to ${UNIT_AMOUNT_MAX}`, {
          field: 'amount',
          reason: 'invalid',
        });
      }
      next.amount = patch.amount;
    }
  }
  if (patch.amountUnit !== undefined) {
    if (patch.amountUnit === null) delete next.amountUnit;
    else {
      if (!isAmountUnit(patch.amountUnit)) {
        fail(400, 'amountUnit must be one of minutes, pages, or problems', {
          field: 'amountUnit',
          reason: 'invalid',
        });
      }
      next.amountUnit = patch.amountUnit;
    }
  }
  if (patch.scheduledDate !== undefined) {
    if (patch.scheduledDate === null) delete next.scheduledDate;
    else next.scheduledDate = checkDateField('scheduledDate', patch.scheduledDate);
  }
  return next;
}

export interface RestoreUnitHost {
  world: World;
  userId: string;
  courseId: string;
  unitId: string;
  tree: PageTree;
  checkCounts: (chapters: number, units: number) => void;
  writePlace: (node: StoredBlock, place: Place) => StoredBlock[];
  tellUnits: (blocks: readonly StoredBlock[]) => void;
}

/** Restores a deleted unit or chapter (with its child units & comments) from trash. */
export function memoryRestoreUnit(host: RestoreUnitHost): CourseUnitDetail {
  const { world, userId, courseId, unitId, tree } = host;
  const deleted = world.deletedBlocks.get(unitId);
  const [top] = deleted?.blocks ?? [];
  if (deleted?.containerId !== courseId || top?.id !== unitId || !isCourseBlock(top)) {
    fail(404, 'no such unit in the trash');
  }
  const nodes = deleted.blocks.filter(isCourseBlock);
  host.checkCounts(
    nodes.filter((block) => block.type === 'chapter').length,
    nodes.filter((block) => block.type === 'unit').length,
  );
  const livingChapter =
    top.type === 'unit' &&
    top.parentId !== '' &&
    world.blocks.get(top.parentId)?.type === 'chapter' &&
    world.blocks.get(top.parentId)?.containerId === courseId;
  const parentId = top.type === 'chapter' ? '' : livingChapter ? top.parentId : '';
  const order = orderOf(top);
  const siblings = tree.siblings(parentId);
  let renumbered: StoredBlock[] = [];
  if (parentId !== top.parentId) {
    renumbered = host.writePlace(top, tree.place('', undefined));
  } else if (siblings.some((sibling) => sibling.order === order)) {
    const before = siblings.filter(
      (sibling) => sibling.order < order || (sibling.order === order && sibling.id < unitId),
    );
    renumbered = host.writePlace(top, tree.place(parentId, before.at(-1)?.id ?? ''));
  }
  top.parentId = parentId;
  world.deletedBlocks.delete(unitId);
  const now = world.now();
  for (const block of deleted.blocks) {
    [block.updatedAt, block.modifiedBy] = [now, userId];
    world.blocks.set(block.id, block);
  }
  host.tellUnits([...renumbered, ...nodes]);
  const comments = deleted.blocks.filter((block) => block.type === 'comment');
  if (comments.length > 0) {
    world.tellContainer(courseId, {
      kind: 'blocks',
      containerId: courseId,
      blocks: comments.map(commentOf),
    });
  }
  return {
    ...unitOf(top),
    body: top.type === 'chapter' ? null : copy(top.body ?? null),
    createdBy: top.createdBy,
    modifiedBy: top.modifiedBy,
  };
}
