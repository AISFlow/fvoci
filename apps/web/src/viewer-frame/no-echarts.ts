/**
 * Stands in for the chart library (echarts) that the PowerPoint renderer
 * imports. The viewer does not draw charts: the worker leaves their parts
 * out (`pptx-parts.ts`), the renderer draws an empty box where each was and
 * the frame labels it (`pptx-clean.ts`). The library is a large part of the
 * renderer's size, so the build puts this file in its place (the alias in
 * vite.config.ts). The renderer registers the chart kinds it would use at
 * load; nothing else is called, and if something is, it says why it cannot
 * draw.
 */
export function use(): void {
  // The kinds the renderer registers are none here.
}

export function init(): never {
  throw new Error('charts are not drawn in this build');
}

/** The chart kinds, components and features the renderer names, none of them anything. */
export const BarChart = {};
export const CandlestickChart = {};
export const CustomChart = {};
export const LineChart = {};
export const PieChart = {};
export const RadarChart = {};
export const ScatterChart = {};
export const AxisPointerComponent = {};
export const GraphicComponent = {};
export const GridComponent = {};
export const LegendComponent = {};
export const RadarComponent = {};
export const TitleComponent = {};
export const TooltipComponent = {};
export const LabelLayout = {};
export const CanvasRenderer = {};
/** The gradients of the renderer's charts. */
export const graphic = {};
