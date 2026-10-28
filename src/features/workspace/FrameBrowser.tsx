import { useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { IconButton, SegmentedControl } from "qing-steel";
import { errorText } from "../../lib/errors";
import { GrayViewer, NumberField } from "./components";
import type { GrayImage } from "./types";
import "./capture.css";

/**
 * 一件的原图浏览（只读）：按相机切换、按帧组翻页。read 的 camera 是 cameras 的下标，seq 从 1 起；
 * source 标识这一件，换件时由调用方换 key 重置相机与帧组。
 */
export function FrameBrowser({ source, label, cameras, groupCount, read }: {
  source: string; label: string; cameras: { id: string; name: string }[]; groupCount: number;
  read: (camera: number, seq: number) => Promise<GrayImage>;
}) {
  const [camera, setCamera] = useState(0);
  const [seq, setSeq] = useState(1);
  const reader = useRef(read); reader.current = read;
  const key = JSON.stringify([source, camera, seq]);
  const [state, setState] = useState<{ key: string; image: GrayImage | null; error: string }>({ key: "", image: null, error: "" });
  useEffect(() => {
    if (!groupCount || !cameras.length) return;
    let alive = true;
    reader.current(camera, seq)
      .then(image => alive && setState({ key, image, error: "" }))
      .catch(e => alive && setState({ key, image: null, error: errorText(e) }));
    return () => { alive = false; };
  }, [key, groupCount, cameras.length]);
  const shown = state.key === key ? state : null;
  const go = (value: number) => setSeq(Math.min(Math.max(1, Number.isFinite(value) ? Math.trunc(value) : 1), Math.max(1, groupCount)));
  const name = cameras[camera]?.name ?? "";
  return <div className="wp-frame-browser">
    <div className="wp-frame-browser-bar">
      {cameras.length > 1 && <SegmentedControl size="sm" label="查看相机" value={String(camera)} onValueChange={value => setCamera(Number(value))}
        options={cameras.map((item, i) => ({ value: String(i), label: item.name }))} />}
      <div className="wp-capture-groups" role="group" aria-label="帧组选择">
        <IconButton size="sm" aria-label="上一帧组" disabled={seq <= 1} onClick={() => go(seq - 1)}><ChevronLeft size={15} /></IconButton>
        <NumberField label="帧组序号" value={seq} min={1} max={Math.max(1, groupCount)} disabled={!groupCount} onChange={go} />
        <IconButton size="sm" aria-label="下一帧组" disabled={seq >= groupCount} onClick={() => go(seq + 1)}><ChevronRight size={15} /></IconButton>
        <span className="muted">/ {groupCount || "—"}</span>
      </div>
    </div>
    <GrayViewer image={shown?.image ?? null} loading={!!groupCount && !shown} error={shown?.error ?? ""}
      label={`${label} · ${name} · 帧组 ${seq}`} emptyTitle="没有图像" emptyHint="这一件没有可读的帧组。" />
  </div>;
}
