/**
 * The highlighter's own worker as it starts (code-highlighter.ts), with a
 * worker of the test's in place of the built one: one whose script fails as
 * it runs, before it says it is ready, is stopped, and no other is started.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tokensFor, useHighlighter } from '$lib/editor/code-highlighter.ts';
import { codeLanguageOf, type CodeLanguage } from '$lib/editor/code-languages.ts';

const made: FailingWorker[] = [];

/** A worker whose script throws as it is run: an error, and never ready. */
class FailingWorker extends EventTarget {
  terminated = false;

  constructor() {
    super();
    made.push(this);
    setTimeout(() => {
      this.dispatchEvent(new Event('error'));
    }, 0);
  }

  postMessage(): void {
    // Its script is not running: nothing answers.
  }

  terminate(): void {
    this.terminated = true;
  }
}

vi.mock('$lib/editor/code-highlight-worker.ts?worker', () => ({ default: FailingWorker }));

const found = codeLanguageOf('ts');
if (!found) throw new Error('no language ts');
const typescript: CodeLanguage = found;

afterEach(() => {
  useHighlighter(undefined);
  made.length = 0;
});

describe('the worker of the highlighter', () => {
  it('is stopped when its script fails before it is ready, and no other starts', async () => {
    useHighlighter(undefined);
    expect(await tokensFor(typescript, 'const a = 1;')).toBeNull();
    expect(made.map((worker) => worker.terminated)).toEqual([true]);
    expect(await tokensFor(typescript, 'const b = 2;')).toBeNull();
    expect(made).toHaveLength(1);
  });
});
