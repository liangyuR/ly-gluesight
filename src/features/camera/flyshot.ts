/** 飞拍速度上限：运动模糊 ≤ 0.5 px，且相邻触发间隔不短于单帧时间（方案文档 2.1 / 2.3）。 */
export function flyshotSpeedLimits({ exposure, mmPerPx, fps, spacing }: { exposure: number; mmPerPx: number; fps: number; spacing: number }) {
  const blur = exposure ? (0.5 * mmPerPx) / (exposure * 1e-6) : 0;
  const frame = spacing * fps;
  return { blur, frame, max: Math.min(blur, frame) };
}
