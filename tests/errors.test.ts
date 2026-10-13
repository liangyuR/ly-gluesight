import { describe, expect, it } from "vitest";
import { errorText } from "../src/lib/errors";

describe("错误文案", () => {
  it.each([
    [new Error("配方不存在"), "配方不存在"],
    ["TypeError: Failed to fetch", "TypeError: Failed to fetch"],
    ["Error: 发布包已损坏", "发布包已损坏"],
  ])("%s → 只去掉 Error: 前缀，保留 TypeError 等类型名", (error, expected) => {
    expect(errorText(error)).toBe(expected);
  });
});
