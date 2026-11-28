import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { subscribe } from "../plc/api";
import type { CameraConfig, Frame } from "./types";

const FRAME_UI_INTERVAL_MS = 250;
type Listener = () => void;
type CameraFrames = {
  id: string;
  frame?: Frame;
  pending?: Frame;
  publishedAt?: number;
  timer?: ReturnType<typeof setTimeout>;
};

const keyFor = (cam: number, id: string) => `${cam}:${id}`;

function createCameraFrameSource() {
  const frames = new Map<number, CameraFrames>();
  const listeners = new Map<string, Set<Listener>>();

  const publish = (cam: number, entry: CameraFrames) => {
    entry.timer = undefined;
    entry.frame = entry.pending;
    entry.pending = undefined;
    entry.publishedAt = performance.now();
    listeners.get(keyFor(cam, entry.id))?.forEach(listener => listener());
  };

  return {
    snapshot(cam: number, id: string) {
      const entry = frames.get(cam);
      return entry?.id === id ? entry.frame : undefined;
    },
    listen(cam: number, id: string, listener: Listener) {
      const key = keyFor(cam, id);
      const group = listeners.get(key) ?? new Set<Listener>();
      group.add(listener);
      listeners.set(key, group);
      return () => {
        group.delete(listener);
        if (!group.size) listeners.delete(key);
      };
    },
    receive(frame: Frame) {
      const cam = frame.cam;
      let entry = frames.get(cam);
      if (entry?.id !== frame.cameraId) {
        clearTimeout(entry?.timer);
        entry = { id: frame.cameraId };
        frames.set(cam, entry);
      }
      const latest = entry.pending ?? entry.frame;
      if (latest && frame.session < latest.session) return;

      if (!listeners.has(keyFor(cam, entry.id))) {
        clearTimeout(entry.timer);
        entry.frame = frame;
        entry.pending = undefined;
        entry.publishedAt = undefined;
        entry.timer = undefined;
        return;
      }
      entry.pending = frame;
      const remaining = entry.publishedAt === undefined || entry.frame?.session !== frame.session
        ? 0 : entry.publishedAt + FRAME_UI_INTERVAL_MS - performance.now();
      if (remaining <= 0) {
        clearTimeout(entry.timer);
        publish(cam, entry);
      } else if (entry.timer === undefined) {
        const scheduled = entry;
        entry.timer = setTimeout(() => publish(cam, scheduled), Math.ceil(remaining));
      }
    },
    retain(configs: readonly CameraConfig[]) {
      for (const [cam, entry] of frames) {
        if (configs[cam]?.id === entry.id) continue;
        clearTimeout(entry.timer);
        frames.delete(cam);
        listeners.get(keyFor(cam, entry.id))?.forEach(listener => listener());
      }
    },
    clear() {
      for (const entry of frames.values()) clearTimeout(entry.timer);
      frames.clear();
    },
  };
}

export type CameraFrameSource = ReturnType<typeof createCameraFrameSource>;

export function useCameraFrames(configs: readonly CameraConfig[]): CameraFrameSource {
  const [source] = useState(createCameraFrameSource);
  const current = useRef(configs);
  current.current = configs;
  useEffect(() => source.retain(configs), [source, configs]);
  useEffect(() => {
    let alive = true;
    const stop = subscribe<Frame>("camera://frame", frame => {
      if (!alive || !frame.cameraId) return;
      if (!current.current.length || current.current[frame.cam]?.id === frame.cameraId) source.receive(frame);
    });
    return () => { alive = false; stop(); source.clear(); };
  }, [source]);
  return source;
}

export function useCameraFrame(source: CameraFrameSource, cam: number, id: string | null) {
  const listen = useCallback((listener: Listener) => id ? source.listen(cam, id, listener) : () => {}, [source, cam, id]);
  const snapshot = useCallback(() => id ? source.snapshot(cam, id) : undefined, [source, cam, id]);
  return useSyncExternalStore(listen, snapshot, snapshot);
}
