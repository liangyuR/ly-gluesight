import { vi } from "vitest";

// jsdom 不做 SVG 坐标变换与指针捕获，拖拽类测试用这些替身把屏幕坐标换成原图坐标。

/** 全局替身：DOMPoint 原样返回、所有 SVG 元素屏幕矩阵为恒等、指针捕获为空操作；不自动还原。 */
export function stubSvgPointer({ pointerEvent = false } = {}) {
  vi.stubGlobal("DOMPoint", class {
    constructor(public x: number, public y: number) {}
    matrixTransform() { return this; }
  });
  if (pointerEvent) vi.stubGlobal("PointerEvent", MouseEvent);
  Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
  Object.defineProperty(SVGElement.prototype, "setPointerCapture", { configurable: true, value: vi.fn() });
}

type Transform<M> = (x: number, y: number, matrix: M) => [number, number];

/** 在 run 期间换上 DOMPoint 替身，结束后还原；不给 transform 时 matrixTransform 原样返回。 */
export function withDOMPoint<M = unknown>(transform: Transform<M> | null, run: () => void) {
  const previous = globalThis.DOMPoint;
  class TestPoint {
    constructor(public x: number, public y: number) {}
    matrixTransform(matrix: M): TestPoint { return transform ? new TestPoint(...transform(this.x, this.y, matrix)) : this; }
  }
  Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: TestPoint });
  try {
    run();
  } finally {
    Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: previous });
  }
}

/** 单个元素的屏幕矩阵替身；inverse 每次调用都会重新求值，测试可中途改变偏移。 */
export function stubScreenCTM(element: Element, inverse: () => object = () => ({})) {
  Object.defineProperty(element, "getScreenCTM", { value: () => ({ inverse }) });
}
