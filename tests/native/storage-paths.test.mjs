import assert from "node:assert/strict";
import { mkdtemp, mkdir, realpath, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { isolatedStorage, normalizedPath } from "./storage-paths.mjs";

test("隔离日志目录解析并拒绝隔离根外的配置", async () => {
  const temp = await mkdtemp(path.join(os.tmpdir(), "gluesight-storage-paths-"));
  try {
    const identifier = "com.example.isolated";
    const data = path.join(temp, identifier);
    const root = path.join(data, "gs_logs");
    const instance = "0123456789abcdef";
    const runtime = path.join(root, "runtime", "test", instance);
    const vision = path.join(root, "vision", "test", instance, "line-1");
    await Promise.all([mkdir(path.join(data, "config"), { recursive: true }), mkdir(path.join(data, "projects", "line-1"), { recursive: true }), mkdir(runtime, { recursive: true }), mkdir(vision, { recursive: true })]);
    await writeFile(path.join(data, "config", "current-project.json"), JSON.stringify({ current: "line-1" }));
    const config = { schema: 1, root, environment: "test", instance, ownerData: data };
    await writeFile(path.join(data, "config", "log-storage.json"), JSON.stringify(config));
    const manifest = { identifier, dataRoot: data };
    const call = async () => ({ root: data, logRoot: root, runtimeLogDir: runtime, visionLogDir: vision });
    const storage = await isolatedStorage(call, manifest, identifier);
    assert.equal(storage.runtime, await realpath(runtime));
    assert.equal(storage.vision, await realpath(vision));
    await writeFile(path.join(data, "config", "log-storage.json"), JSON.stringify({ ...config, root: temp }));
    await assert.rejects(() => isolatedStorage(call, manifest, identifier));
    await writeFile(path.join(data, "config", "log-storage.json"), JSON.stringify({ ...config, ownerData: temp }));
    await assert.rejects(() => isolatedStorage(call, manifest, identifier));
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
});

test("Windows 扩展路径与普通路径指向同一录制目录", () => {
  assert.equal(normalizedPath("\\\\?\\C:\\tests\\records", "win32"), normalizedPath("c:\\tests\\records", "win32"));
  assert.equal(normalizedPath("\\\\?\\UNC\\server\\share\\records", "win32"), normalizedPath("\\\\server\\share\\records", "win32"));
  assert.throws(() => normalizedPath("\\\\?\\Volume{123}\\records", "win32"));
});
