import { expect, it } from "vitest";
import { canImportSampleGroup } from "../src/features/workspace/teach";
import type { ShotSpec } from "../src/features/cycle/types";

const shot = (enabled: boolean[], skip = false): ShotSpec => ({
  id: "P1", poseId: "Pose1", camera: "cam1", view: 1, bead: "J1", skip, path: [],
  views: enabled.map((on, i) => ({ view: i + 1, enabled: on, path: [] })),
});

it.each([
  ["整圈采集后每个拍照点只选 1 幅", [shot([true, false, false]), shot([false, true, false])], true],
  ["某个拍照点选中 2 幅", [shot([true, false, false]), shot([true, true, false])], false],
  ["选中多幅的只是不检拍照点", [shot([true, false, false]), shot([true, true, true], true)], true],
])("%s → 允许导入原图样本组为 %s", (_name, shots, expected) => {
  expect(canImportSampleGroup(shots)).toBe(expected);
});
