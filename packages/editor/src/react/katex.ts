import { useEffect, useState } from "react";
import {
	type MathRender,
	mathMlWithoutKatex,
	renderMathMl,
} from "../math-ml.js";

export function useMathMl(latex: string, display = true): MathRender {
	const [render, setRender] = useState<MathRender>({
		html: null,
		failed: false,
	});
	useEffect(() => {
		const settled = mathMlWithoutKatex(latex);
		if (settled) {
			setRender(settled);
			return;
		}
		let alive = true;
		void renderMathMl(latex, display).then((next) => {
			if (alive) setRender(next);
		});
		return () => {
			alive = false;
		};
	}, [latex, display]);
	return render;
}
