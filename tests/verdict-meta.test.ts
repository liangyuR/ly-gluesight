import { expect, it } from "vitest";
import { verdictTone } from "../src/features/history/meta";
import type { Verdict } from "../src/features/cycle/types";

// 历史列表行、历史分段卡片、在线检测判定卡共用这份映射：合格带警告、产品 NG、检测异常三者语义色分开。
it.each<{ verdict: Verdict; tone: string }>([
  { verdict: "OK", tone: "ok" },
  { verdict: "OK_WITH_EXCURSION", tone: "warn" },
  { verdict: "NG_POSITION", tone: "ng" },
  { verdict: "NG_WIDTH", tone: "ng" },
  { verdict: "NG_ABSOLUTE", tone: "ng" },
  { verdict: "NG_GAP", tone: "ng" },
  { verdict: "ERR_INSPECT", tone: "fault" },
])("判定 $verdict → 语义色 $tone", ({ verdict, tone }) => {
  expect(verdictTone(verdict)).toBe(tone);
});
