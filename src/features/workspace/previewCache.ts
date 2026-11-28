import { createFrameReadQueue, frameReadAbortError } from "../../lib/frameReadQueue";
import type { FrameImage } from "./types";

const budget = 24 * 1024 * 1024;
const entries = new Map<string, { image: FrameImage; size: number; expires: number }>();
let used = 0;
export const scheduleFrameRead = createFrameReadQueue(2);

function remember(key: string, image: FrameImage) {
  const size = image.pixels?.data.byteLength ?? image.url.length * 2;
  if (size > budget) return;
  const previous = entries.get(key);
  if (previous) { used -= previous.size; entries.delete(key); }
  entries.set(key, { image, size, expires: Date.now() + 30_000 }); used += size;
  while (used > budget || entries.size > 24) {
    const first = entries.entries().next().value!;
    used -= first[1].size; entries.delete(first[0]);
  }
}

export function loadPreview(key: string, read: () => Promise<FrameImage>, signal?: AbortSignal): Promise<FrameImage> {
  if (signal?.aborted) return Promise.reject(frameReadAbortError());
  const cached = entries.get(key);
  if (cached) {
    entries.delete(key);
    if (cached.expires > Date.now()) { entries.set(key, cached); return Promise.resolve(cached.image); }
    used -= cached.size;
  }
  return scheduleFrameRead(async () => {
    const image = await read();
    remember(key, image);
    return image;
  }, signal);
}
