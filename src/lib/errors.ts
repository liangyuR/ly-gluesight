export function errorText(error: unknown): string {
  const text = error instanceof Error ? error.message : typeof error === "string" ? error : String(error ?? "");
  const trimmed = text.replace(/^Error:\s*/, "").trim();
  return trimmed || "操作未完成，原因未知";
}
