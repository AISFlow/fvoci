import { describe, expect, it } from 'vitest';

import { examAverage, examScoreCount, type ExamScores } from '../../src/lib/logic/exam-results.ts';

const unpublished: ExamScores = {
  averageScore: null,
  scoreCount: 0,
  resultsPublishedAt: null,
  publishedScoreBucket: null,
};

describe('examAverage', () => {
  it('tells nothing without scores', () => {
    expect(examAverage(unpublished)).toEqual({ kind: 'none' });
  });

  it('holds the average of scores not published', () => {
    expect(examAverage({ ...unpublished, scoreCount: 3 })).toEqual({ kind: 'held' });
  });

  it('tells the published snapshot with how many had scored at least', () => {
    expect(
      examAverage({
        averageScore: 86.5,
        scoreCount: 12,
        resultsPublishedAt: 1_800_000_000_000,
        publishedScoreBucket: 10,
      }),
    ).toEqual({ kind: 'published', average: 86.5, atLeast: 10 });
  });

  it('tells an average given without a snapshot as it is', () => {
    expect(examAverage({ ...unpublished, averageScore: 90, scoreCount: 3 })).toEqual({
      kind: 'shown',
      average: 90,
    });
  });
});

describe('examScoreCount', () => {
  it('is exact for who manages the course', () => {
    expect(examScoreCount({ ...unpublished, scoreCount: 7 }, true)).toEqual({
      count: 7,
      atLeast: false,
    });
  });

  it('is a floor for everybody else, but none is none', () => {
    expect(examScoreCount({ ...unpublished, scoreCount: 5 }, false)).toEqual({
      count: 5,
      atLeast: true,
    });
    expect(examScoreCount(unpublished, false)).toEqual({ count: 0, atLeast: false });
  });
});
