import {
	findParentNodeClosestToPos,
	InputRule,
	mergeAttributes,
	Node,
} from "@tiptap/core";

/*
 * WHY: #638 — remark-math 의 `math` 를 문단으로 눕히면 `?format=md` 등가가 수식에서만 깨진다.
 * 공식 `@tiptap/extension-mathematics` 는 MIT 지만 `import katex from "katex"` 가 정적이라
 * 에디터 청크에 katex(76 KB gz)가 무조건 들어간다 — 문서 라우트 예산(#616) 여유는 23 KB gz다.
 * 그래서 mermaid 와 같은 최소 atom 블록을 두고, 렌더만 첫 수식에서 지연 import 한다.
 */
export const MathBlock = Node.create({
	name: "math",
	group: "block",
	atom: true,
	addAttributes() {
		// WHY: LaTeX 는 <pre> 의 텍스트 자식으로 나간다 — 속성으로 한 번 더 적을 이유가 없다.
		return { latex: { default: "", rendered: false } };
	},
	parseHTML() {
		return [
			{
				tag: "[data-math]",
				getAttrs: (el) => {
					if (!(el instanceof HTMLElement)) return false;
					return { latex: el.textContent ?? "" };
				},
			},
		];
	},
	renderHTML({ node, HTMLAttributes }) {
		const latex = typeof node.attrs.latex === "string" ? node.attrs.latex : "";
		return ["pre", mergeAttributes(HTMLAttributes, { "data-math": "" }), latex];
	},
	addInputRules() {
		return [
			new InputRule({
				find: /^\$\$[\s\n]$/,
				handler: ({ chain, range }) => {
					chain()
						.deleteRange(range)
						.insertContent({ type: this.name, attrs: { latex: "" } })
						.run();
				},
			}),
		];
	},
});

/*
 * WHY: #688 — 본문 한가운데의 `$x$` 는 블록으로 눕힐 수 없다. 렌더·예산 사정은 위와 같아
 * 같은 지연 import 를 쓰고, 노드는 latex 하나만 든 인라인 아톰이다.
 */
export const MathInline = Node.create({
	name: "mathInline",
	group: "inline",
	inline: true,
	atom: true,
	addAttributes() {
		// WHY: LaTeX 는 <span> 의 텍스트 자식으로 나간다 — 속성으로 한 번 더 적을 이유가 없다.
		return { latex: { default: "", rendered: false } };
	},
	parseHTML() {
		return [
			{
				tag: "span[data-math-inline]",
				getAttrs: (el) => {
					if (!(el instanceof HTMLElement)) return false;
					return { latex: el.textContent ?? "" };
				},
			},
		];
	},
	renderHTML({ node, HTMLAttributes }) {
		const latex = typeof node.attrs.latex === "string" ? node.attrs.latex : "";
		return [
			"span",
			mergeAttributes(HTMLAttributes, { "data-math-inline": "" }),
			latex,
		];
	},
	addStorage() {
		/* WHY: #707 — addInputRules 핸들러는 virtual "\n" 만 받고 원래 KeyboardEvent 를
		 * 못 본다(Shift 여부 불가). addKeyboardShortcuts 는 이벤트를 보므로, 같은 확장 안에서
		 * 그 keymap 플러그인이 inputRulesPlugin 보다 먼저 도는 점(ExtensionManager.plugins)을
		 * 이용해 이 플래그로 넘긴다. */
		return { pendingShiftEnter: false };
	},
	addKeyboardShortcuts() {
		return {
			"Shift-Enter": () => {
				this.storage.pendingShiftEnter = true;
				return false;
			},
			Enter: () => {
				this.storage.pendingShiftEnter = false;
				return false;
			},
		};
	},
	addInputRules() {
		return [
			new InputRule({
				/* WHY: #688 — Pandoc 규칙 전부를 타이핑 시점에 건다: 여는 `$` 뒤·닫는 `$` 앞이
				 * 공백이 아니고, **닫는 `$` 다음 글자**가 숫자도 `$` 도 아니어야 한다. 그 글자는
				 * 아직 안 쳤으므로 규칙이 한 글자 더 기다린다 — `$5-$10`·`$10~$20`·`$5,$10` 은
				 * 그 자리에 숫자가 와서 영영 열리지 않는다. `${var}` 도 파서와 같이 뺀다.
				 * 그 자리에 숫자가 와서 영영 열리지 않고, `$x$ `·`$x$,` 는 열린다. */
				find: /\$([^\s${](?:[^$]*[^\s$])?)\$([^\d$])$/,
				handler: ({ chain, range, match, state }) => {
					const latex = match[1];
					const tail = match[2];
					if (latex === undefined || tail === undefined) return;
					/* WHY: #701/#707 — Tiptap 코어(`inputRulesPlugin.handleKeyDown`)는 Enter 를
					 * 문서에 넣지 않는 가상의 "\n" 입력으로 흉내 내 입력 규칙을 다시 확인한다. 그
					 * tail 이 "\n" 이면 실제로 친 글자가 아니라 Enter(또는 Shift+Enter) 그 자체다 —
					 * 숫자 절은 뒤 글자가 없어 자동으로 참이다. Shift+Enter 는 위 keymap 이 미리 세운
					 * 플래그로 구분해 hardBreak 를 유지하고, 목록 항목 안의 plain Enter 는
					 * splitListItem 으로 새 항목을 낸다(그 밖은 기존대로 splitBlock). 코드 마크·코드
					 * 블록 배제는 run$1 의 공통 가드가 이미 건다. */
					if (tail === "\n") {
						const isShiftEnter = this.storage.pendingShiftEnter;
						this.storage.pendingShiftEnter = false;
						const $pos = state.doc.resolve(range.from);
						/* WHY: #707 리뷰 — listItem 뿐 아니라 TaskItem(`taskItem`)도 같은
						 * splitListItem 경로를 쓴다. 찾은 조상의 실제 타입명을 그대로 넘겨야
						 * 체크박스 등 taskItem 속성이 새 항목에도 이어진다. */
						const listAncestor = findParentNodeClosestToPos(
							$pos,
							(node) =>
								node.type.name === "listItem" || node.type.name === "taskItem",
						);
						const staged = chain()
							.deleteRange(range)
							.insertContent({ type: this.name, attrs: { latex } });
						if (isShiftEnter) {
							staged.setHardBreak().run();
							return;
						}
						if (listAncestor) {
							staged.splitListItem(listAncestor.node.type.name).run();
							return;
						}
						staged.splitBlock().run();
						return;
					}
					/* WHY: range 는 뒤따라 친 글자까지 덮는다 — 노드 뒤에 그 글자를 되돌려 놓는다. */
					chain()
						.deleteRange(range)
						.insertContent([
							{ type: this.name, attrs: { latex } },
							{ type: "text", text: tail },
						])
						.run();
				},
			}),
		];
	},
});
