/**
 * 图表（ECharts，按需引入，历史详情、统计等共用）：只注册用到的图表与组件，控制打包体积。
 * ECharts 画在 canvas 上，主题色要先从 CSS 变量取成实际颜色（`themeColor`）。
 */
import { useEffect, useRef } from "react";
import * as echarts from "echarts/core";
import { BarChart, LineChart, ScatterChart } from "echarts/charts";
import { DataZoomComponent, GridComponent, LegendComponent, MarkAreaComponent, MarkLineComponent, TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";

echarts.use([LineChart, BarChart, ScatterChart, GridComponent, TooltipComponent, LegendComponent, MarkLineComponent, MarkAreaComponent, DataZoomComponent, CanvasRenderer]);

export type ChartOption = echarts.EChartsCoreOption;
export type ChartInstance = echarts.ECharts;

/** CSS 变量（如 `--primary`）的实际颜色；取不到时用 fallback。 */
export function themeColor(name: string, fallback: string): string {
  if (typeof document === "undefined") return fallback;
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

let canvas2d: boolean | undefined;
/** 能拿到 2D canvas 上下文（jsdom 拿不到，ECharts 在那里初始化后清屏会报错）。 */
function canvasSupported(): boolean {
  if (canvas2d === undefined) {
    try { canvas2d = typeof HTMLCanvasElement !== "undefined" && !/jsdom/i.test(navigator.userAgent) && !!document.createElement("canvas").getContext("2d"); }
    catch { canvas2d = false; }
  }
  return canvas2d;
}

/**
 * 一张图：option 变了整份替换（notMerge），容器尺寸变了自动 resize。`onReady` 拿到实例挂事件，返回的函数在卸载时调用。
 * 环境不支持 canvas（如测试里的 jsdom）时不画，只留容器。
 */
export function Chart({ option, onReady, className, label, height = 220 }: {
  option: ChartOption;
  onReady?: (chart: ChartInstance) => void | (() => void);
  className?: string;
  label: string;
  height?: number;
}) {
  const box = useRef<HTMLDivElement>(null);
  const chart = useRef<ChartInstance | null>(null);
  const ready = useRef(onReady);
  ready.current = onReady;
  useEffect(() => {
    const element = box.current;
    if (!element) return;
    if (!canvasSupported()) return;
    let instance: ChartInstance;
    try { instance = echarts.init(element, undefined, { renderer: "canvas" }); }
    catch { return; }
    chart.current = instance;
    const cleanup = ready.current?.(instance);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => instance.resize());
    observer?.observe(element);
    return () => { observer?.disconnect(); if (typeof cleanup === "function") cleanup(); instance.dispose(); chart.current = null; };
  }, []);
  useEffect(() => {
    try { chart.current?.setOption(option, { notMerge: true }); } catch { /* 不支持 canvas 的环境 */ }
  }, [option]);
  return <div ref={box} className={className} role="img" aria-label={label} style={{ width: "100%", height }} />;
}
