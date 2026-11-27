import { act, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { expect, it, vi } from "vitest";
import { usePlcValues } from "../src/features/plc/api";
import type { PointValue } from "../src/features/plc/types";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

type Points = Record<string, PointValue>;
const point = (value: number): PointValue => ({ value, ts: 1, error: null, errorKind: null });

function Values() {
  const values = usePlcValues();
  return <>{Object.entries(values).map(([id, point]) => <output aria-label={id} key={id}>{String(point.value)}</output>)}</>;
}

function startRead() {
  const registration = deferred<() => void>();
  const initial = deferred<Points>();
  const stop = vi.fn();
  let emit = (_: Points) => {};
  vi.mocked(listen).mockImplementation((_event, listener) => {
    emit = values => (listener as (event: { payload: Points }) => void)({ payload: values });
    return registration.promise;
  });
  vi.mocked(invoke).mockReturnValue(initial.promise);
  const view = render(<Values />);
  return {
    forward: (values: Points) => act(() => emit(values)),
    register: () => act(async () => registration.resolve(stop)),
    initial, stop, view,
  };
}

it("事件先更新部分点位 → 迟到首读只补齐其余点位", async () => {
  const read = startRead();
  expect(invoke).not.toHaveBeenCalled();
  await read.register();
  expect(invoke).toHaveBeenCalledExactlyOnceWith("plc_get_values", undefined);
  read.forward({ A: point(2) });
  read.forward({ A: point(3) });
  await act(async () => read.initial.resolve({ A: point(1), B: point(9) }));
  expect(screen.getByLabelText("A")).toHaveTextContent("3");
  expect(screen.getByLabelText("B")).toHaveTextContent("9");
});

it("初始读取期间清空点位 → 只显示清空后的增量", async () => {
  const read = startRead();
  await read.register();
  read.forward({ A: point(2) });
  read.forward({});
  expect(screen.queryByLabelText("A")).not.toBeInTheDocument();
  read.forward({ C: point(7) });
  await act(async () => read.initial.resolve({ A: point(1), B: point(9) }));
  expect(screen.getByLabelText("C")).toHaveTextContent("7");
  expect(screen.queryByLabelText("A")).not.toBeInTheDocument();
  expect(screen.queryByLabelText("B")).not.toBeInTheDocument();
});

it("已收到点位事件但首读失败 → 保留事件值", async () => {
  const read = startRead();
  await read.register();
  read.forward({ A: point(2) });
  await act(async () => read.initial.reject(new Error("首读失败")));
  expect(screen.getByLabelText("A")).toHaveTextContent("2");
});

it("监听建立前离页 → 释放迟到监听且省去首读", async () => {
  const read = startRead();
  read.view.unmount();
  await read.register();
  expect(read.stop).toHaveBeenCalledOnce();
  expect(invoke).not.toHaveBeenCalled();
});
