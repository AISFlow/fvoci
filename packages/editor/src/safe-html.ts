/** WHY: sanitize 가 끝난 HTML 만 이 타입을 얻는다 — 생산자 = asSafeHtml 호출자 전부(grep 으로 감사). */
export type SafeHtml = string & { readonly __brand: "SafeHtml" };

export function asSafeHtml(html: string): SafeHtml {
  return html as SafeHtml;
}
