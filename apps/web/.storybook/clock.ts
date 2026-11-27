import { STORY_NOW } from '../stories/fixtures.ts';

// Only the Storybook iframe imports this. Timers and performance.now keep running.
// Preserve Date's multi-argument constructors, parsing and instanceof behavior.
const SystemDate = Date;
globalThis.Date = new Proxy(SystemDate, {
  construct(target, args: unknown[], newTarget): Date {
    return Reflect.construct(target, args.length === 0 ? [STORY_NOW] : args, newTarget) as Date;
  },
  apply(): string {
    return new SystemDate(STORY_NOW).toString();
  },
  get(target, property, receiver): unknown {
    return property === 'now'
      ? () => STORY_NOW
      : (Reflect.get(target, property, receiver) as unknown);
  },
});
