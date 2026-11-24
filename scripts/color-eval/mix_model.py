"""离线混合轴模型。RGB 使用原图的 0–255 数值，坐标均与原图对齐。"""

from __future__ import annotations

import cv2
import numpy as np


VALID, HIGHLIGHT, DARK, OFF_AXIS, OUTSIDE = range(5)


def _rgb(values, ndim):
    image = np.asarray(values, dtype=np.float32)
    if image.ndim != ndim or image.shape[-1] != 3 or not image.size:
        raise ValueError("需要非空的 RGB 像素数组")
    if not np.isfinite(image).all() or image.min() < 0 or image.max() > 255:
        raise ValueError("RGB 像素必须为 0–255 有限值")
    return image


def _mask(values, shape):
    mask = np.asarray(values)
    if mask.dtype != np.bool_ or mask.shape != shape:
        raise ValueError("取样掩膜必须与图像尺寸一致并使用布尔值")
    return mask


def _light_masks(rgb, dark_level, highlight_level):
    luminance = 0.299 * rgb[..., 0] + 0.587 * rgb[..., 1] + 0.114 * rgb[..., 2]
    return luminance < dark_level, rgb.max(axis=-1) >= highlight_level


def _check_radius(radius):
    if isinstance(radius, bool) or not isinstance(radius, (int, np.integer)) or radius < 0:
        raise ValueError("高光膨胀半径必须是非负整数像素")


def _image_light_masks(image, dark_level, highlight_level, radius):
    _check_radius(radius)
    dark, saturated = _light_masks(image, dark_level, highlight_level)
    if radius and saturated.any():
        h, w = saturated.shape
        kernel = np.ones((2 * min(radius, h - 1) + 1, 2 * min(radius, w - 1) + 1), np.uint8)
        saturated = cv2.dilate(saturated.astype(np.uint8), kernel).astype(bool)
    return dark, saturated


def usable_sample_mask(rgb, *, dark_level=12, highlight_level=250, highlight_radius_px=1):
    """先在完整去马赛克帧排除高光及其插值邻域，再与各类别的训练掩膜求交。"""
    image = _rgb(rgb, 3)
    dark, saturated = _image_light_masks(image, dark_level, highlight_level, highlight_radius_px)
    return ~(dark | saturated)


def _chroma(rgb):
    log_rgb = np.log1p(rgb)
    return np.stack((log_rgb[..., 0] - log_rgb[..., 1],
                     log_rgb[..., 2] - log_rgb[..., 1]), axis=-1)


def _project(chroma, green, yellow, blue):
    displacement = chroma - green
    ends = np.stack((yellow - green, blue - green))
    lengths_sq = np.sum(ends * ends, axis=-1)
    fractions = np.clip((displacement @ ends.T) / lengths_sq, 0, 1)
    yellow_error = displacement - fractions[..., 0, None] * ends[0]
    blue_error = displacement - fractions[..., 1, None] * ends[1]
    yellow_sq = np.sum(yellow_error * yellow_error, axis=-1)
    blue_sq = np.sum(blue_error * blue_error, axis=-1)
    toward_blue = blue_sq < yellow_sq
    axis = np.where(toward_blue, fractions[..., 1], -fractions[..., 0])
    distance = np.sqrt(np.minimum(yellow_sq, blue_sq))
    return axis, distance


def fit_mix_model(green, yellow, blue, background, *, dark_level=12, highlight_level=250,
                  color_floor=0.02, highlight_radius_px=1):
    """输入经 usable_sample_mask 筛选的四组 (N,3) 样本，输出可冻结的模型。"""
    _check_radius(highlight_radius_px)
    if not 0 <= dark_level < highlight_level <= 255:
        raise ValueError("亮度下限和通道饱和上限需满足 0 ≤ 下限 < 上限 ≤ 255")
    if not np.isfinite(color_floor) or color_floor <= 0:
        raise ValueError("色度离散下限必须是有限正数")
    samples = {}
    centers = {}
    counts = {}
    for name, values in (("green", green), ("yellow", yellow), ("blue", blue),
                         ("background", background)):
        pixels = _rgb(values, 2)
        dark, saturated = _light_masks(pixels, dark_level, highlight_level)
        usable = pixels[~(dark | saturated)]
        if len(usable) < 3:
            raise ValueError(f"{name} 的有效取样不足 3 个像素")
        samples[name] = _chroma(usable)
        centers[name] = np.median(samples[name], axis=0)
        counts[name] = len(usable)

    green_center, yellow_center, blue_center = (centers[key] for key in ("green", "yellow", "blue"))
    for endpoint, other in ((yellow_center, blue_center), (blue_center, yellow_center)):
        segment = other - green_center
        length_sq = float(segment @ segment)
        if length_sq <= color_floor ** 2:
            raise ValueError("纯色与合格色的距离不足以建立混合轴")
        t = np.clip(((endpoint - green_center) @ segment) / length_sq, 0, 1)
        if np.linalg.norm(endpoint - (green_center + t * segment)) <= color_floor:
            raise ValueError("黄、蓝两侧混合轴重叠，方向无法可靠区分")

    axis, distance = _project(samples["green"], green_center, yellow_center, blue_center)
    off_axis_scale = max(float(np.quantile(distance, 0.68)), color_floor)
    axis_scale = float(np.quantile(np.abs(axis), 0.68))
    background_scale = np.maximum(
        1.4826 * np.median(np.abs(samples["background"] - centers["background"]), axis=0),
        color_floor,
    )
    return {
        "version": 1,
        "space": "log_chroma",
        "centers": {key: center.tolist() for key, center in centers.items()},
        "validSamples": counts,
        "offAxisScale": off_axis_scale,
        "axisScale": axis_scale,
        "backgroundScale": background_scale.tolist(),
        "darkLevel": float(dark_level),
        "highlightLevel": float(highlight_level),
        "highlightRadiusPx": int(highlight_radius_px),
        "colorFloor": float(color_floor),
    }


def compute_mix_map(rgb, model, *, roi=None, off_axis_limit=3.5):
    """返回原图尺寸的 axis、off_axis、state、score，离轴像素保留投影供诊断。"""
    if not np.isfinite(off_axis_limit) or off_axis_limit <= 0:
        raise ValueError("离轴量门限必须是有限正数")
    image = _rgb(rgb, 3)
    centers = model["centers"]
    green, yellow, blue = (np.asarray(centers[key], dtype=np.float32)
                           for key in ("green", "yellow", "blue"))
    axis, distance = _project(_chroma(image), green, yellow, blue)
    off_axis = distance / model["offAxisScale"]
    dark, saturated = _image_light_masks(
        image, model["darkLevel"], model["highlightLevel"], model["highlightRadiusPx"],
    )
    state = np.where(off_axis <= off_axis_limit, VALID, OFF_AXIS).astype(np.uint8)
    state[dark] = DARK
    state[saturated] = HIGHLIGHT
    if roi is not None:
        state[~_mask(roi, state.shape)] = OUTSIDE
    unavailable = (state != VALID) & (state != OFF_AXIS)
    axis[unavailable] = np.nan
    off_axis[unavailable] = np.nan
    score = np.exp(-0.5 * off_axis ** 2)
    score[state == OFF_AXIS] = 0
    return {"axis": axis, "off_axis": off_axis, "state": state, "score": score}


def measure_background(rgb, mask, model):
    """在指定背景区独立测量色度漂移，归一位移供上层结合胶面偏移判定。"""
    image = _rgb(rgb, 3)
    region = _mask(mask, image.shape[:2])
    dark, saturated = _image_light_masks(
        image, model["darkLevel"], model["highlightLevel"], model["highlightRadiusPx"],
    )
    usable = image[region & ~(dark | saturated)]
    result = {"validPixels": len(usable), "totalPixels": int(region.sum()),
              "delta": None, "normalizedShift": None}
    if len(usable):
        delta = np.median(_chroma(usable), axis=0) - model["centers"]["background"]
        result["delta"] = delta.tolist()
        result["normalizedShift"] = float(np.linalg.norm(delta / model["backgroundScale"]))
    return result
