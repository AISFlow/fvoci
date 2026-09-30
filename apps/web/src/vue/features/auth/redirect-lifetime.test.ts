import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { runInNewContext } from "node:vm";
import { computed, defineComponent, effectScope, nextTick, ref, watchEffect } from "vue";
import {
  createMemoryHistory,
  createRouter,
  isNavigationFailure,
  NavigationFailureType,
  type Router,
} from "vue-router";

function deferred<T>() {
  let resolve: ((value: T) => void) | undefined;
  let reject: ((error: Error) => void) | undefined;
  const promise = new Promise<T>((accept, fail) => {
    resolve = accept;
    reject = fail;
  });
  return {
    promise,
    resolve(value: T) {
      assert.ok(resolve);
      resolve(value);
    },
    reject(error: Error) {
      assert.ok(reject);
      reject(error);
    },
  };
}

// Execute the actual page watcher with real Vue effect cleanup and router chunk loading.
// This targets delayed navigation failure ownership, not an HTTP/authentication substitute.
for (const page of ["SetupPage", "ConsentPage"]) {
  const source = readFileSync(path.join(import.meta.dirname, "../../pages", `${page}.vue`), "utf8");
  const effect = /watchEffect\(\(onCleanup\) => \{[\s\S]*?\n\}\);/.exec(source)?.[0];
  assert.ok(effect, `${page}: watcher not found`);

  const scenarios = [
    "active rejection",
    "unmounted",
    "route changed",
    "route changed and returned",
    "eligibility changed",
    "aborted",
  ];
  if (page === "SetupPage") scenarios.push("setup loading", "setup error");

  for (const scenario of scenarios) {
    await test(`${page}: delayed login chunk ${scenario}`, async () => {
      const component = defineComponent({ render: () => null });
      const chunk = deferred<typeof component>();
      const started = deferred<undefined>();
      const router = createRouter({
        history: createMemoryHistory(),
        routes: [
          { path: "/setup", name: "setup", component },
          { path: "/consent", name: "consent", component },
          { path: "/other", component },
          {
            path: "/login",
            component: () => {
              started.resolve(undefined);
              return chunk.promise;
            },
          },
        ],
      });
      const errors: unknown[] = [];
      router.onError((error) => errors.push(error));
      await router.push(page === "SetupPage" ? "/setup" : "/consent");
      if (scenario === "aborted") router.beforeEach((to) => to.path !== "/login");
      const replace = router.replace.bind(router);
      const navigations: ReturnType<Router["replace"]>[] = [];
      router.replace = (to) => {
        const navigation = replace(to);
        navigations.push(navigation);
        return navigation;
      };
      const replaced: string[] = [];
      const assigned: string[] = [];
      const setup = { isLoading: ref(false), isError: ref(false), data: ref({ needed: false }) };
      const authState = ref(true);
      const unauthorized = computed(() => authState.value);
      const scope = effectScope();
      try {
        scope.run(() => {
          runInNewContext(effect, {
            watchEffect,
            router,
            window: {
              location: {
                replace: (target: string) => replaced.push(target),
                assign: (target: string) => assigned.push(target),
              },
            },
            setup,
            unauthorized,
            pending: { data: ref(undefined) },
            returnTo: ref("/"),
          });
        });
        if (scenario === "aborted") {
          const result = await navigations[0];
          assert.ok(isNavigationFailure(result, NavigationFailureType.aborted));
          await nextTick();
          assert.deepEqual(replaced, []);
          assert.deepEqual(errors, []);
          return;
        }

        await started.promise;
        if (scenario === "unmounted") scope.stop();
        if (scenario === "route changed" || scenario === "route changed and returned") {
          // Stop only the effect that starts a redirect, so returning does not start a new one.
          if (scenario === "route changed and returned") scope.stop();
          await router.push("/other");
          if (scenario === "route changed and returned")
            await router.push(page === "SetupPage" ? "/setup" : "/consent");
        }
        if (scenario === "eligibility changed") {
          setup.data.value = { needed: true };
          authState.value = false;
        }
        if (scenario === "setup loading") setup.isLoading.value = true;
        if (scenario === "setup error") setup.isError.value = true;
        await nextTick();
        const failure = new Error("delayed login chunk failure");
        chunk.reject(failure);
        const outcomes = await Promise.allSettled(navigations);
        await nextTick();
        assert.ok(outcomes.some((result) => result.status === "rejected"));
        assert.ok(errors.includes(failure));
        assert.deepEqual(assigned, []);
        assert.deepEqual(replaced, scenario === "active rejection" ? ["/login"] : []);
      } finally {
        scope.stop();
      }
    });
  }
}
