import { useEffect, useState } from "react";
import { asSafeHtml, type SafeHtml } from "./safe-html.js";

/*
 * WHY: #638 · #616 — katex 는 76 KB gz 다. 정적으로 끌면 문서 라우트 예산(여유 23 KB gz)이
 * 그 자리에서 터진다. 첫 수식 노드가 마운트될 때만 받는다.
 * 출력은 MathML — 브라우저 네이티브라 katex CSS 24 KB 와 폰트 60개(.ttf 20개, web dist 가드가
 * 금지)를 하나도 싣지 않는다. `trust: false` 라 SafeHtml 계약(safe-html.tsx 주석)을 만족한다.
 */

/* WHY: #656 F9 — 1 MB latex 는 renderToString 이 메인 스레드를 2.9 초 잡고 11 MB HTML 을 만든다.
 * 협업 문서에서 한 명이 동료 탭을 얼릴 수 있어 렌더 전에 자른다. maxSize 는 `\rule{99999em}` 류가
 * 거대한 <mspace> 로 레이아웃을 밀어내는 것을 막는다(매크로 폭탄은 katex 기본 maxExpand 가 막는다). */
const MAX_LATEX = 10_000;
const MAX_SIZE = 100;

/* WHY: #738 — nonce 가 붙은 style-src 아래에서 style= 속성은 CSP3 §6.7.3.3 상 nonce 로
 * 구제되지 않는다. output:"mathml" 에서 katex 가 style= 을 내는 곳은 셋뿐이고 전부 장식이다 —
 * \pmb(text-shadow) · \fcolorbox(border) · 그리고 렌더 전체가 중단되는 오류(`{`·`\frac{`)의
 * <span class="katex-error" style="color:#cc0000">. 식 내부 오류(\thisisnotacommand 등)는
 * <mstyle mathcolor> 즉 속성이라 애초에 CSP 밖이다. 간격·레이아웃도 전부 MathML 속성이라
 * 스트립에 안 무너진다. 파서로 걷어내고 오류 색은 .katex-error 규칙이 준다 — \pmb 의 굵기
 * 강조와 \fcolorbox 의 테두리는 style= 이 유일한 표현이라 이 정책 아래서는 지원하지 않는다. */
function withoutStyleAttributes(html: string): string {
	const parsed = new DOMParser().parseFromString(html, "text/html");
	for (const el of parsed.body.querySelectorAll("[style]"))
		el.removeAttribute("style");
	return parsed.body.innerHTML;
}

export type MathRender = { html: SafeHtml | null; failed: boolean };

export function useMathMl(latex: string, display = true): MathRender {
	const [render, setRender] = useState<MathRender>({
		html: null,
		failed: false,
	});
	useEffect(() => {
		const tooLong = latex.length > MAX_LATEX;
		if (latex.trim() === "" || tooLong) {
			setRender({ html: null, failed: tooLong });
			return;
		}
		let alive = true;
		/* WHY: #656 F8 — throwOnError:false 여도 katex 는 던진다(중첩 중괄호 2000 개 → RangeError).
		 * 청크 fetch 실패(재배포 후 stale·오프라인)도 같은 자리로 온다. 잡지 않으면 unhandled
		 * rejection 이 나고 노드는 아무 표시 없이 원문에 머문다. */
		void import("katex")
			.then(({ default: katex }) =>
				asSafeHtml(
					withoutStyleAttributes(
						katex.renderToString(latex, {
							displayMode: display,
							output: "mathml",
							throwOnError: false,
							trust: false,
							maxSize: MAX_SIZE,
						}),
					),
				),
			)
			.then((html) => {
				if (alive) setRender({ html, failed: false });
			})
			.catch(() => {
				if (alive) setRender({ html: null, failed: true });
			});
		return () => {
			alive = false;
		};
	}, [latex, display]);
	return render;
}
