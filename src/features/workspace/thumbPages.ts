import { errorText } from "../../lib/errors";
import type { ThumbStrip } from "./workbenchDraft";

export const THUMB_PAGE_SIZE = 32;
type Page = { key: string; round: string; camera: string; start: number; count: number; visible: boolean };
type Watch = { pages: Page[]; attempted: Set<string>; notify: () => void };
type Entry = { strip?: ThumbStrip; error?: string; bytes: number };
export type ThumbFrame = { pixels: Uint8Array | null; width: number; height: number; channels: 1 | 3; status: "loading" | "ready" | "missing" | "error" };
const pageKey = (round: string, camera: string, start: number) => JSON.stringify([round, camera, start]);

export class ThumbPages {
  private entries = new Map<string, Entry>();
  private watches = new Set<Watch>();
  private active = new Set<string>();
  private used = 0;

  constructor(private read: (round: string, camera: string, startSeq: number, count: number) => Promise<ThumbStrip>, private budget = 32 * 1024 * 1024) {}

  watch(round: string, cameras: string[], count: number, start: number, end: number, notify: () => void) {
    const pages: Page[] = [];
    if (round && count > 0 && end > start) {
      const first = Math.floor(Math.max(0, start) / THUMB_PAGE_SIZE);
      const last = Math.floor((Math.min(count, end) - 1) / THUMB_PAGE_SIZE);
      const add = (page: number, visible: boolean) => {
        const offset = page * THUMB_PAGE_SIZE;
        if (offset < 0 || offset >= count) return;
        for (const camera of cameras) pages.push({ key: pageKey(round, camera, offset), round, camera, start: offset, count: Math.min(THUMB_PAGE_SIZE, count - offset), visible });
      };
      for (let page = first; page <= last; page++) add(page, true);
      add(first - 1, false);
      add(last + 1, false);
    }
    const watch = { pages, attempted: new Set<string>(), notify };
    this.watches.add(watch);
    this.pump();
    return () => { this.watches.delete(watch); };
  }

  frame(round: string, camera: string, index: number): ThumbFrame {
    const start = Math.floor(index / THUMB_PAGE_SIZE) * THUMB_PAGE_SIZE;
    const key = pageKey(round, camera, start);
    const entry = this.entries.get(key);
    if (entry) { this.entries.delete(key); this.entries.set(key, entry); }
    const strip = entry?.strip;
    const pixels = strip?.frames[index - start] ?? null;
    return { pixels, width: strip?.width ?? 0, height: strip?.height ?? 0, channels: strip?.channels ?? 1,
      status: entry?.error ? "error" : !strip ? "loading" : pixels ? "ready" : "missing" };
  }

  error(round: string, cameras: string[], start: number, end: number) {
    for (let page = Math.floor(start / THUMB_PAGE_SIZE); page * THUMB_PAGE_SIZE < end; page++) {
      for (const camera of cameras) {
        const error = this.entries.get(pageKey(round, camera, page * THUMB_PAGE_SIZE))?.error;
        if (error) return error;
      }
    }
    return "";
  }

  retry(round: string) {
    for (const [key, entry] of this.entries) if (entry.error && JSON.parse(key)[0] === round) this.entries.delete(key);
    for (const watch of this.watches) {
      for (const page of watch.pages) if (page.round === round && !this.entries.has(page.key)) watch.attempted.delete(page.key);
    }
    this.pump();
  }

  private remember(key: string, entry: Entry) {
    const visible = new Set([...this.watches].flatMap(watch => watch.pages.filter(page => page.visible).map(page => page.key)));
    const previous = this.entries.get(key);
    if (previous) { this.used -= previous.bytes; this.entries.delete(key); }
    while (this.used + entry.bytes > this.budget || this.entries.size >= 256) {
      const oldest = [...this.entries].find(([candidate]) => !visible.has(candidate));
      if (!oldest) {
        if (!visible.has(key)) return;
        entry = { error: "可见缩略图超过缓存容量", bytes: 0 };
        break;
      }
      this.used -= oldest[1].bytes;
      this.entries.delete(oldest[0]);
    }
    this.entries.set(key, entry);
    this.used += entry.bytes;
  }

  private pump() {
    const wanted = [...this.watches].flatMap(watch => watch.pages.map(page => ({ watch, page }))).sort((a, b) => Number(b.page.visible) - Number(a.page.visible));
    for (const { watch, page } of wanted) {
      if (this.active.size >= 2) break;
      if (this.entries.has(page.key) || this.active.has(page.key) || watch.attempted.has(page.key)) continue;
      for (const owner of this.watches) if (owner.pages.some(item => item.key === page.key)) owner.attempted.add(page.key);
      this.active.add(page.key);
      void Promise.resolve().then(() => this.read(page.round, page.camera, page.start + 1, page.count))
        .then(strip => ({ strip, bytes: [...new Set(strip.frames.flatMap(frame => frame ? [frame.buffer] : []))].reduce((size, buffer) => size + buffer.byteLength, 16 + strip.frames.length) } as Entry), error => ({ error: errorText(error), bytes: 0 } as Entry))
        .then(entry => {
          const owners = [...this.watches].filter(owner => owner.pages.some(item => item.key === page.key));
          if (owners.length) {
            this.remember(page.key, entry);
            for (const owner of owners) owner.notify();
          }
        }).finally(() => { this.active.delete(page.key); this.pump(); });
    }
  }
}
