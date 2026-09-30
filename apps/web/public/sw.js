// apps/web/public/sw.js
/*
 * WHY: 번들을 타지 않는 순수 파일이다 — 서비스 워커는 해시 없는 루트 경로(`/sw.js`)로 받아야
 * 스코프가 `/` 가 되고, 모든 화면에서 온 알림 클릭을 이 하나가 받는다(#455-c).
 */

/* WHY: 새 워커가 옛 워커의 종료를 기다리면 배포 뒤 첫 푸시가 옛 코드로 처리된다. */
self.addEventListener("install", () => {
  self.skipWaiting();
});
self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

/** WHY: 페이로드는 서버가 만든 JSON 이지만, 깨진 프레임 하나로 워커가 죽으면 안 된다. */
function readPayload(data) {
  try {
    const parsed = data ? data.json() : null;
    if (parsed && typeof parsed.title === "string") return parsed;
  } catch {
    /* 아래 기본값으로 떨어진다 */
  }
  return { title: "FVOCI", body: "", url: "/" };
}

self.addEventListener("push", (event) => {
  const { title, body, url } = readPayload(event.data);
  event.waitUntil(
    self.registration.showNotification(title, {
      body,
      data: { url },
    }),
  );
});

/*
 * WHY: 라우팅 규칙 — 이미 그 주소를 연 탭이 있으면 그 탭으로, 없고 창이 열려 있으면 그 창을
 * 옮기고, 아무것도 없을 때만 새 창을 연다. 클릭마다 탭이 늘어나는 게 가장 흔한 불만이다.
 */
self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const target = new URL(event.notification.data?.url ?? "/", self.location.origin).href;
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({
        type: "window",
        includeUncontrolled: true,
      });
      const same = windows.find((client) => client.url === target);
      if (same) return same.focus();
      const open = windows[0];
      if (open && typeof open.navigate === "function") {
        const moved = await open.navigate(target);
        return (moved ?? open).focus();
      }
      return self.clients.openWindow(target);
    })(),
  );
});
