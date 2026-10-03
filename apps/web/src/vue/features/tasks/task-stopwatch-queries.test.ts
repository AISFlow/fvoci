import { describe, expect, test } from "bun:test";
import { QueryClient } from "@tanstack/query-core";
import { QueryClient as VueQueryClient, useQuery } from "@tanstack/vue-query";
import { effectScope, nextTick, ref, watch } from "vue";
import { ProblemError } from "@/lib/api";
import {
  captureTimerDenial,
  captureTimerQuery,
  captureTimerRead,
  removeCapturedTimerQuery,
  TimerReadFailure,
} from "./task-stopwatch-queries";

const key = ["task-timer", "actor", "S1", "workspace", "task"] as const;
function client() {
  return new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
}
async function deniedQuery(cache: QueryClient) {
  const error = new TimerReadFailure(
    new ProblemError(409, "conflict", undefined, "timer_context_changed"),
    key,
  );
  try {
    await cache.query({
      queryKey: key,
      queryFn: () => Promise.reject(error),
    });
  } catch (caught) {
    expect(caught).toBe(error);
  }
  return error;
}

describe("timer read origin and component lifetime", () => {
  test("both installed Vue observers recognize their shared denial; a settled successor rejects the old error", async () => {
    const cache = new VueQueryClient({
      defaultOptions: { queries: { retry: false, gcTime: Infinity } },
    });
    const scope = effectScope();
    const session = ref("S1");
    let denyS1 = false;
    const retired: { observer: string; session: string }[] = [];
    const mounted = scope.run(() => {
      const options = () => {
        const capturedKey = ["task-timer", "actor", session.value, "workspace", "task"] as const;
        return {
          queryKey: capturedKey,
          staleTime: 0,
          queryFn: () =>
            captureTimerRead(capturedKey, () =>
              denyS1 && capturedKey[2] === "S1"
                ? Promise.reject(
                    new ProblemError(409, "conflict", undefined, "timer_context_changed"),
                  )
                : Promise.resolve({ session: capturedKey[2] }),
            ),
        };
      };
      const left = useQuery(options, cache);
      const right = useQuery(options, cache);
      for (const [observer, result] of [
        ["left", left],
        ["right", right],
      ] as const) {
        watch(
          [
            () => result.error.value,
            () => result.dataUpdatedAt.value,
            () => result.status.value,
            () => result.fetchStatus.value,
          ],
          () => {
            const capturedKey = [
              "task-timer",
              "actor",
              session.value,
              "workspace",
              "task",
            ] as const;
            const capture = captureTimerDenial(
              cache,
              result.error.value,
              capturedKey,
              result.status.value,
              result.fetchStatus.value,
            );
            if (capture) retired.push({ observer, session: capture.queryKey[2] ?? "" });
          },
          { flush: "pre" },
        );
      }
      return { left, right };
    });
    if (!mounted) throw new Error("missing installed observer scope");
    try {
      await mounted.left.refetch();
      denyS1 = true;
      await mounted.left.refetch();
      await nextTick();
      expect(new Set(retired.map((value) => value.observer))).toEqual(new Set(["left", "right"]));
      session.value = "S2";
      await nextTick();
      await mounted.left.refetch();
      await nextTick();
      expect(mounted.left.data.value).toEqual({ session: "S2" });
      expect(mounted.right.data.value).toEqual({ session: "S2" });
      expect(retired.every((value) => value.session === "S1")).toBe(true);
    } finally {
      scope.stop();
      cache.clear();
    }
  });

  test("asynchronous authority errors retain an immutable originating key; network errors stay distinct", async () => {
    const mutableKey: string[] = [...key];
    const original = new ProblemError(404, "not_found");
    let caught: unknown;
    try {
      await captureTimerRead(mutableKey, () => Promise.reject(original));
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(TimerReadFailure);
    const scoped = caught as TimerReadFailure;
    mutableKey[2] = "S2";
    expect(scoped.queryKey).toEqual(key);
    expect(Object.isFrozen(scoped.queryKey)).toBe(true);
    expect(scoped.status).toBe(404);
    const network = new TypeError("connection failed");
    let networkFailure: unknown;
    try {
      await captureTimerRead(key, () => Promise.reject(network));
    } catch (error) {
      networkFailure = error;
    }
    expect(networkFailure).toBe(network);
  });

  test("a partial old error cannot retire a new session, fetching result or replacement query", async () => {
    const cache = client();
    try {
      const error = await deniedQuery(cache);
      expect(captureTimerDenial(cache, error, key, "error", "idle")?.query).toBe(
        cache.getQueryCache().find({ queryKey: key }),
      );
      expect(
        captureTimerDenial(
          cache,
          error,
          ["task-timer", "actor", "S2", "workspace", "task"],
          "error",
          "idle",
        ),
      ).toBe(undefined);
      expect(captureTimerDenial(cache, error, key, "error", "fetching")).toBe(undefined);
      cache.removeQueries({ queryKey: key, exact: true });
      cache.setQueryData(key, { authorized: "replacement" });
      expect(captureTimerDenial(cache, error, key, "error", "idle")).toBe(undefined);
      expect(cache.getQueryData<{ authorized: string }>(key)).toEqual({
        authorized: "replacement",
      });
    } finally {
      cache.clear();
    }
  });

  test("a same-key replacement survives cancellation completion after an ABA scope transition", async () => {
    const cache = client();
    let release = () => {};
    const cancellationReturned = new Promise<void>((resolve) => {
      release = resolve;
    });
    const cancel = cache.cancelQueries.bind(cache);
    cache.cancelQueries = async (filters, options) => {
      await cancel(filters, options);
      await cancellationReturned;
    };
    try {
      cache.setQueryData(key, { private: "old" });
      const captured = captureTimerQuery(cache, key);
      let lifetime = 1;
      const capturedLifetime = lifetime;
      const retiring = removeCapturedTimerQuery(
        cache,
        captured,
        () => lifetime === capturedLifetime,
      );
      lifetime++; // S1 -> S2
      cache.removeQueries({ queryKey: key, exact: true });
      cache.setQueryData(key, { authorized: "new" });
      lifetime++; // S2 -> S1, same primitive key, different lifetime/query
      release();
      expect(await retiring).toBe(false);
      expect(cache.getQueryData<{ authorized: string }>(key)).toEqual({ authorized: "new" });
      expect(cache.getQueryCache().find({ queryKey: key })).not.toBe(captured.query);
    } finally {
      release();
      cache.clear();
    }
  });

  test("query replacement alone also survives; current exact retirement preserves an unrelated cached query", async () => {
    const cache = client();
    try {
      cache.setQueryData(key, { private: "old" });
      const old = captureTimerQuery(cache, key);
      cache.removeQueries({ queryKey: key, exact: true });
      cache.setQueryData(key, { authorized: "new" });
      expect(await removeCapturedTimerQuery(cache, old, () => true)).toBe(false);
      const sentinel = ["task-timer", "actor", "S1", "workspace", "other"] as const;
      cache.setQueryData(sentinel, { mounted: "unchanged" });
      const current = captureTimerQuery(cache, key);
      expect(await removeCapturedTimerQuery(cache, current, () => true)).toBe(true);
      expect(cache.getQueryData<{ authorized: string }>(key)).toBe(undefined);
      expect(cache.getQueryData<{ mounted: string }>(sentinel)).toEqual({ mounted: "unchanged" });
    } finally {
      cache.clear();
    }
  });
});
