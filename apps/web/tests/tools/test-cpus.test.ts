import { describe, expect, it } from 'vitest';

import { playwrightWorkers, testCpus, vitestWorkers } from '../../tools/test-cpus.ts';

const given = (value: string) => ({ FVOCI_TEST_CPUS: value });

describe('FVOCI_TEST_CPUS', () => {
  it('is a whole number of one or more, or not set', () => {
    expect(testCpus(given(' 6 '))).toBe(6);
    expect(testCpus({})).toBeUndefined();
    expect(testCpus(given(''))).toBeUndefined();
    for (const wrong of ['0', '-2', '+4', '1.5', 'four', '4x']) {
      expect(() => testCpus(given(wrong)), wrong).toThrow(/FVOCI_TEST_CPUS/);
    }
  });

  it('leaves every tool as it was when it is not set', () => {
    expect(playwrightWorkers(undefined, {})).toBeUndefined();
    expect(playwrightWorkers(4, {})).toBe(4);
    expect(vitestWorkers({})).toBeUndefined();
  });

  it('gives Playwright half the processors, and never more than a config fixes', () => {
    expect(playwrightWorkers(undefined, given('8'))).toBe(4);
    expect(playwrightWorkers(undefined, given('1'))).toBe(1);
    expect(playwrightWorkers(2, given('8'))).toBe(2);
    expect(playwrightWorkers(4, given('6'))).toBe(3);
  });

  it('gives Vitest one less than the processors, and at least one', () => {
    expect(vitestWorkers(given('8'))).toBe(7);
    expect(vitestWorkers(given('1'))).toBe(1);
  });
});
