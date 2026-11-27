import { expect, it } from "vitest";
import { buildPartEvidence } from "../src/features/cycle/partEvidence";
import { burstPartView, burstResult } from "./cycle-visual-fixtures";
import { pathSegment } from "./fixtures";

it("工件判定 OK 且检测不完整 → 按区域保留覆盖问题，只定位应测相机", () => {
  const { burst } = burstPartView();
  const result = burstResult();
  const judgement = result.burst!;
  const sample = judgement.regions[0];
  const skipped = { segment: 1, fromSeq: 1, toSeq: 4 };
  judgement.uninspected = [skipped];
  judgement.regions = [
    { ...sample, region: { kind: { type: "skipped", segment: 1 }, firstSeq: 1, lastSeq: 4 }, incompleteGroups: [], offsetUnjudged: null },
    { ...sample, region: { kind: { type: "straight", segment: 1 }, firstSeq: 5, lastSeq: 7 }, incompleteGroups: [5, 6], offsetUnjudged: "noFrames" },
    { ...sample, region: { kind: { type: "unsegmented", firstSegment: 1, lastSegment: 2, reason: "transition" }, firstSeq: 8, lastSeq: 12 }, incompleteGroups: [8], offsetUnjudged: null },
  ];
  burst.groups[1].cells[1] = { status: "missing", verdict: null, frameCounter: null };
  burst.groups[4].cells[2] = { status: "missing", verdict: null, frameCounter: null };
  burst.groups[5].cells[0] = { status: "notInspected", verdict: null, frameCounter: 1006 };
  burst.groups[7].cells[1] = { status: "missing", verdict: null, frameCounter: null };
  const segment = pathSegment(1, "straight", {
    cameras: ["CAM-2"], offsetRef: { measure: "absoluteX", camera: "CAM-2", xRef: 0, spread: null },
  });

  const evidence = buildPartEvidence(result, burst, [segment, pathSegment(2, "corner", { cameras: ["CAM-1"] })]);

  expect(evidence.coverage.status).toBe("incomplete");
  expect(evidence.coverage.affected).toEqual([
    { region: judgement.regions[1].region, sequences: [5, 6], locateSeq: 5, cameras: [{ camera: 1, name: "CAM-2", status: "missing", sequences: [5] }] },
    { region: judgement.regions[2].region, sequences: [8], locateSeq: 8, cameras: [] },
  ]);
  expect(evidence.coverage.offsetUnjudged).toEqual([{ region: judgement.regions[1].region, reason: "noFrames", camera: "CAM-2" }]);
  expect(evidence.coverage.uninspected).toEqual([skipped]);
  expect(evidence.defects).toEqual([]);
  expect(buildPartEvidence(result, burst, []).coverage.affected[0]).toEqual({
    region: judgement.regions[1].region, sequences: [5, 6], locateSeq: 5, cameras: [],
  });
});

it("区域 NG 与单帧 NG 并存 → 区分区域复核起点和同结论的实际异常帧", () => {
  const { burst } = burstPartView();
  const result = burstResult();
  const judgement = result.burst!;
  const sample = judgement.regions[0];
  result.verdict = judgement.verdict = "NG_GAP";
  judgement.regions = [
    { ...sample, region: { kind: { type: "skipped", segment: 1 }, firstSeq: 1, lastSeq: 3 }, verdict: "OK" },
    { ...sample, region: { kind: { type: "straight", segment: 1 }, firstSeq: 4, lastSeq: 7 }, verdict: "NG_WIDTH", reason: "胶宽不均" },
    { ...sample, region: { kind: { type: "straight", segment: 2 }, firstSeq: 8, lastSeq: 12 }, verdict: "NG_GAP", reason: "断胶" },
  ];
  burst.groups[1].cells[0].verdict = "NG_WIDTH";
  burst.groups[8].cells[1].verdict = "NG_GAP";
  burst.groups[9].cells[0].verdict = "NG_GAP";
  burst.groups = burst.groups.filter(group => group.seq !== 4);

  const evidence = buildPartEvidence(result, burst, [pathSegment(1, "straight"), pathSegment(2, "straight")]);

  expect(evidence.defects).toEqual([
    { region: judgement.regions[1].region, verdict: "NG_WIDTH", reason: "胶宽不均", frame: null, locateSeq: 5 },
    { region: judgement.regions[2].region, verdict: "NG_GAP", reason: "断胶", frame: { seq: 9, camera: 1, name: "CAM-2" }, locateSeq: 9 },
  ]);
});
