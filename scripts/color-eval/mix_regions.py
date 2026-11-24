"""在像素坐标的胶路上量测混胶偏色条带，供离线可行性评估使用。"""

import cv2
import numpy as np
from scipy.ndimage import find_objects
from scipy.spatial import cKDTree

from mix_model import DARK, HIGHLIGHT, OFF_AXIS, OUTSIDE, VALID


def _stations(line):
    points = np.asarray(line, dtype=np.float64)
    if points.ndim != 2 or points.shape[1] != 2 or len(points) < 2 or not np.isfinite(points).all():
        raise ValueError("胶路需要至少两个有限的 [x, y] 像素点")
    lengths = np.linalg.norm(np.diff(points, axis=0), axis=1)
    points = points[np.r_[True, lengths > 0]]
    along = np.r_[0.0, np.cumsum(lengths[lengths > 0])]
    if len(points) < 2:
        raise ValueError("胶路长度为零")
    s = np.linspace(0, along[-1], int(np.ceil(along[-1])) + 1)
    centers = np.column_stack([np.interp(s, along, coordinate) for coordinate in points.T])
    tangent = np.gradient(centers, axis=0)
    norm = np.linalg.norm(tangent, axis=1)
    if (norm < 1e-9).any():
        raise ValueError("胶路原地折返，无法确定该站的横向方向")
    tangent /= norm[:, None]
    normals = np.column_stack((-tangent[:, 1], tangent[:, 0]))
    edges = np.r_[0.0, (s[:-1] + s[1:]) / 2, s[-1]]
    return s, centers, tangent, normals, edges


def _sample(axial, state, centers, normals, offsets):
    xy = np.floor(centers[:, None] + normals[:, None] * offsets[None, :, None] + 0.5).astype(np.intp)
    x, y = xy[..., 0], xy[..., 1]
    inside = (x >= 0) & (x < state.shape[1]) & (y >= 0) & (y < state.shape[0])
    strip_state = np.full(x.shape, OUTSIDE, np.uint8)
    strip_state[inside] = state[y[inside], x[inside]]
    values = np.zeros(x.shape, dtype=np.float64)
    valid = strip_state == VALID
    values[valid] = axial[y[valid], x[valid]]
    return values, strip_state


def _smooth(values, valid, radius):
    """在同一横向位置、连续可测的胶面内做滑动平均。"""
    if radius == 0:
        return values
    rows = np.arange(len(values))[:, None]
    first = np.maximum.accumulate(np.where(valid, 0, rows + 1), axis=0)
    last = np.minimum.accumulate(np.where(valid, len(values), rows)[::-1], axis=0)[::-1]
    start = np.maximum(rows - radius, first)
    stop = np.minimum(rows + radius + 1, last)
    sums = np.pad(np.cumsum(values, axis=0), ((1, 0), (0, 0)))
    total = np.take_along_axis(sums, stop, axis=0) - np.take_along_axis(sums, start, axis=0)
    return np.divide(total, stop - start, out=np.zeros_like(total), where=valid)


def _join_highlights(labels, count, strip_state, edges, max_gap):
    if max_gap == 0 or count <= 2:
        return labels
    parents = np.arange(count)

    def root(label):
        while label != parents[label]:
            parents[label] = parents[parents[label]]
            label = parents[label]
        return label

    highlight = np.pad((strip_state == HIGHLIGHT).T.astype(np.int8), ((0, 0), (1, 1)))
    changes = np.diff(highlight, axis=1)
    columns, starts = np.nonzero(changes == 1)
    _, stops = np.nonzero(changes == -1)
    eligible = (starts > 0) & (stops < len(labels)) & (edges[stops] - edges[starts] < max_gap)
    for column, start, stop in zip(columns[eligible], starts[eligible], stops[eligible]):
        before, after = labels[start - 1, column], labels[stop, column]
        if before and after and before != after:
            parents[root(after)] = root(before)
    return np.array([root(i) for i in range(count)], dtype=labels.dtype)[labels]


def _source_pixels(state, centers, tangent, normals, half):
    """逐个原图像素映射回条带格，弯道内外侧的面积都按原图像素计。"""
    search = np.zeros(state.shape, dtype=np.uint8)
    cv2.polylines(search, [np.rint(centers).astype(np.int32)], False, 1, thickness=2 * half + 5)
    y, x = np.nonzero((search > 0) & (state == VALID))
    xy = np.column_stack((x, y))
    # 与取样的 [n-0.5, n+0.5) 像素格约定一致，格边上的等距查询取较小原图坐标。
    _, row = cKDTree(centers).query(xy - 1e-9)
    delta = xy - centers[row]
    across = np.einsum("ij,ij->i", delta - 1e-9, normals[row])
    along = np.einsum("ij,ij->i", delta, tangent[row])
    col = np.floor(across + 0.5).astype(np.intp) + half
    inside = ((col >= 0) & (col <= 2 * half) &
              ((row != 0) | (along >= -1e-8)) &
              ((row != len(centers) - 1) | (along <= 1e-8)))
    return y[inside], x[inside], row[inside], col[inside]


def _regions(smoothed, strip_state, edges, source_pixels, shape, threshold, max_gap):
    regions = []
    y, x, row, col = source_pixels
    for sign, direction in ((-1, "yellow"), (1, "blue")):
        detected = (strip_state == VALID) & (sign * smoothed >= threshold)
        count, labels = cv2.connectedComponents(detected.astype(np.uint8), connectivity=8)
        if count == 1:
            continue
        labels = _join_highlights(labels, count, strip_state, edges, max_gap)
        image_labels = np.zeros(shape, dtype=np.int32)
        image_labels[y, x] = labels[row, col]
        strip_boxes = find_objects(labels, max_label=count - 1)
        image_boxes = find_objects(image_labels, max_label=count - 1)
        for label, (strip_box, image_box) in enumerate(zip(strip_boxes, image_boxes), start=1):
            if strip_box is None or image_box is None:
                continue
            measured = labels[strip_box] == label
            start, stop = strip_box[0].start, strip_box[0].stop
            row_widths = np.diff(np.pad(measured.astype(np.int8), ((0, 0), (1, 1))), axis=1)
            widths = np.nonzero(row_widths == -1)[1] - np.nonzero(row_widths == 1)[1]
            image_region = (image_labels[image_box] == label).astype(np.uint8)
            contours, _ = cv2.findContours(
                image_region, cv2.RETR_LIST, cv2.CHAIN_APPROX_SIMPLE,
                offset=(image_box[1].start, image_box[0].start),
            )
            regions.append({
                "direction": direction,
                "contours": [contour[:, 0, :].tolist() for contour in contours],
                "startPx": float(edges[start]),
                "endPx": float(edges[stop]),
                "confirmedLengthPx": float(np.diff(edges[start:stop + 1])[measured.any(axis=1)].sum()),
                "spanPx": float(edges[stop] - edges[start]),
                "maxWidthPx": int(widths.max()),
                "areaPx2": int(image_region.sum()),
            })
    return regions


def measure_regions(axial, state, line, *, half_width_px, smooth_px, axis_threshold,
                    max_highlight_gap_px=0):
    """输出沿单条有向胶路的 regions 与 stations，原图坐标及距离、面积均以像素计。

    validFraction 的分母为本站整个搜索截面的采样数（包括底材、不可测和图外），
    不能直接解读为实际胶面的可测比例。medianAxial 缺观测时为 None。
    """
    axial, state = np.asarray(axial), np.asarray(state)
    if axial.ndim != 2 or axial.shape != state.shape or axial.size == 0:
        raise ValueError("轴向图与有效性图须为相同尺寸的非空单通道数组")
    if not np.isin(state, (VALID, HIGHLIGHT, DARK, OFF_AXIS, OUTSIDE)).all():
        raise ValueError("有效性图包含未知状态")
    if not np.isfinite(axial[state == VALID]).all():
        raise ValueError("可测胶面的轴向偏移必须有限")
    if not np.isfinite([half_width_px, smooth_px, axis_threshold, max_highlight_gap_px]).all():
        raise ValueError("条带量测参数必须有限")
    if half_width_px < 1 or smooth_px < 0 or max_highlight_gap_px < 0 or not 0 < axis_threshold <= 1:
        raise ValueError("搜索半宽至少 1 px，平滑与合并长度非负，轴向门限在 (0,1] 内")
    half = int(np.floor(half_width_px))
    s, centers, tangent, normals, edges = _stations(line)
    values, strip_state = _sample(axial, state, centers, normals, np.arange(-half, half + 1))
    valid = strip_state == VALID
    smoothed = _smooth(values, valid, min(len(s), int(np.floor(smooth_px / (2 * s[1])))))
    pixels = _source_pixels(state, centers, tangent, normals, half)
    regions = _regions(smoothed, strip_state, edges, pixels, state.shape, axis_threshold,
                       max_highlight_gap_px)
    stations = []
    for i, samples in enumerate(strip_state):
        surface = smoothed[i, valid[i]]
        stations.append({
            "sPx": float(s[i]),
            "center": centers[i].tolist(),
            "medianAxial": float(np.median(surface)) if surface.size else None,
            "surfacePixels": int(surface.size),
            "corridorSamples": int(samples.size),
            "validFraction": float(surface.size / samples.size),
            "highlightSamples": int(np.count_nonzero(samples == HIGHLIGHT)),
            "darkSamples": int(np.count_nonzero(samples == DARK)),
            "offAxisSamples": int(np.count_nonzero(samples == OFF_AXIS)),
            "outsideSamples": int(np.count_nonzero(samples == OUTSIDE)),
        })
    return {"regions": regions, "stations": stations}
