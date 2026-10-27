import assert from "node:assert/strict";
import { readFile, realpath } from "node:fs/promises";
import path from "node:path";

export function normalizedPath(value, platform = process.platform) {
  if (platform !== "win32") return path.resolve(value);
  let plain = value;
  if (/^\\\\\?\\UNC\\/i.test(plain)) plain = `\\\\${plain.slice(8)}`;
  else if (/^\\\\\?\\[a-z]:\\/i.test(plain)) plain = plain.slice(4);
  else assert(!plain.startsWith("\\\\?\\"), `不支持的扩展路径：${plain}`);
  return path.win32.resolve(plain).toLowerCase();
}
export const samePath = (left, right) => normalizedPath(left) === normalizedPath(right);
const inside = (parent, child) => {
  const relative = path.relative(normalizedPath(parent), normalizedPath(child));
  return relative !== "" && relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
};

export async function isolatedStorage(call, manifest, identifier) {
  assert.equal(manifest.identifier, identifier, "仅允许隔离测试实例");
  assert(typeof manifest.dataRoot === "string" && path.isAbsolute(manifest.dataRoot), "缺少隔离资料目录");
  const data = await realpath(manifest.dataRoot);
  assert.equal(path.basename(data).toLowerCase(), identifier.toLowerCase(), "资料目录不属于隔离实例");
  const [status, configText, pointerText] = await Promise.all([
    call("storage_status"),
    readFile(path.join(data, "config", "log-storage.json"), "utf8"),
    readFile(path.join(data, "config", "current-project.json"), "utf8"),
  ]);
  assert(samePath(await realpath(status.root), data), "应用资料目录与隔离实例不一致");
  const config = JSON.parse(configText.replace(/^\uFEFF/, ""));
  const projectId = JSON.parse(pointerText.replace(/^\uFEFF/, "")).current;
  assert(config.schema === 1 && config.environment === "test" && /^[a-f0-9]{16}$/i.test(config.instance), "日志命名空间无效");
  assert(typeof config.root === "string" && path.isAbsolute(config.root), "日志根无效");
  assert(typeof config.ownerData === "string" && path.isAbsolute(config.ownerData) && samePath(await realpath(config.ownerData), data), "日志绑定不属于隔离资料目录");
  assert(typeof projectId === "string" && /^[a-z0-9_-]{1,64}$/i.test(projectId), "项目编号无效");
  const logRoot = await realpath(config.root);
  assert(inside(data, logRoot), "测试日志根必须位于隔离资料目录中");
  assert(samePath(await realpath(status.logRoot), logRoot), "应用日志根与配置不一致");
  const runtime = await realpath(status.runtimeLogDir);
  const vision = await realpath(status.visionLogDir);
  assert(samePath(runtime, path.join(logRoot, "runtime", "test", config.instance)), "运行日志路径不匹配");
  assert(samePath(vision, path.join(logRoot, "vision", "test", config.instance, projectId)), "视觉日志路径不匹配");
  const project = path.join(data, "projects", projectId);
  assert(samePath(await realpath(project), project), "项目目录不能指向其他位置");
  return { data, project, runtime, vision };
}

export async function isolatedDirectory(parent, name) {
  const expected = path.join(parent, name);
  const actual = await realpath(expected);
  assert(inside(parent, actual) && samePath(actual, expected), `目录超出隔离范围：${actual}`);
  return actual;
}
