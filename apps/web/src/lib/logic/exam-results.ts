/**
 * What the card of a course exam says of everybody's scores, as the server
 * gives them: an average is told only in the snapshot a manager of the course
 * published (rounded, with how many scored counted down to fives), and how
 * many scored is exact only for who manages the course (`exact`), counted
 * down to fives for everybody else.
 */
export interface ExamScores {
  averageScore: number | null;
  scoreCount: number;
  resultsPublishedAt: number | null;
  publishedScoreBucket: number | null;
}

export type ExamAverage =
  /** The published snapshot: its average, and how many had scored, counted down to fives. */
  | { kind: 'published'; average: number; atLeast: number | null }
  /** An average told without a snapshot, by a backend that publishes none. */
  | { kind: 'shown'; average: number }
  /** There are scores, and no average is told of them yet. */
  | { kind: 'held' }
  | { kind: 'none' };

export function examAverage(exam: ExamScores): ExamAverage {
  if (exam.averageScore === null) return exam.scoreCount > 0 ? { kind: 'held' } : { kind: 'none' };
  return exam.resultsPublishedAt === null
    ? { kind: 'shown', average: exam.averageScore }
    : { kind: 'published', average: exam.averageScore, atLeast: exam.publishedScoreBucket };
}

/** How many have scored, and whether that is a floor (counted down to fives) rather than the count. */
export function examScoreCount(
  exam: ExamScores,
  exact: boolean,
): { count: number; atLeast: boolean } {
  return { count: exam.scoreCount, atLeast: !exact && exam.scoreCount > 0 };
}

/** The fewest scores the server publishes a snapshot of. */
export const FEWEST_PUBLISHED_SCORES = 5;
