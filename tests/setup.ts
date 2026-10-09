import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

// jsdom 不进行布局；图表保留组件内的默认尺寸。实际缩放需要浏览器测试。
vi.stubGlobal("ResizeObserver", class {
  observe() {}
  unobserve() {}
  disconnect() {}
});
Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, value: () => {} });
Object.defineProperties(HTMLDialogElement.prototype, {
  showModal: { configurable: true, value(this: HTMLDialogElement) { this.open = true; } },
  close: { configurable: true, value(this: HTMLDialogElement) { this.open = false; } },
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  sessionStorage.clear();
});
