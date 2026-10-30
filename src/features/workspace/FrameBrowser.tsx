import { useEffect, useRef, useState } from "react";
import { errorText } from "../../lib/errors";
import { GrayViewer } from "./components";
import { FrameGrid } from "./FrameGrid";
import type { GrayImage } from "./types";
import "./capture.css";

/**
 * 一件的原图浏览（只读）：在相机 × 帧组格子里点选。read 的 camera 是 cameras 的下标，seq 从 1 起；
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
  const name = cameras[camera]?.name ?? "";
  return <div className="wp-frame-browser">
    {groupCount > 0 && <FrameGrid label="相机与帧组" cameras={cameras} groupCount={groupCount} camera={camera} seq={seq}
      onSelect={(c, s) => { setCamera(c); setSeq(s); }} />}
    <GrayViewer image={shown?.image ?? null} loading={!!groupCount && !shown} error={shown?.error ?? ""}
      label={`${label} · ${name} · 帧组 ${seq}`} emptyTitle="没有图像" emptyHint="这一件没有可读的帧组。" />
  </div>;
}
