import { Alert, Button, Dialog, Stack } from "qing-steel";
import type { ProjectMeta } from "../../lib/project";
import { useProjectSwitch, useProjectUsage, useUnsavedCandidate } from "./hooks";
import { recordsText, usageText } from "./rules";

export default function SwitchProjectDialog({ target, onClose }: { target: ProjectMeta; onClose: () => void }) {
  const usage = useProjectUsage([target.id])[target.id];
  const unsaved = useUnsavedCandidate();
  const { restarting, restartMessage, error, run } = useProjectSwitch();
  return <Dialog open title="切换项目" closeDisabled={restarting} onOpenChange={open => { if (!open && !restarting) onClose(); }}
    footer={<><Button disabled={restarting} onClick={onClose}>取消</Button><Button variant="filled" tone="warn" loading={restarting && !restartMessage} disabled={restarting || !!unsaved} onClick={() => void run(target.id)}>{restartMessage ? "等待手动重启" : restarting ? "正在重启…" : "切换并重启"}</Button></>}>
    <Stack direction="column" align="stretch" gap={12} wrap={false}>
      <dl className="kv">
        <dt>项目</dt><dd>{target.name}</dd>
        <dt>编号</dt><dd className="mono">{target.id}</dd>
        <dt>配方</dt><dd>{usageText(usage, u => `${u.recipes} 个`)}</dd>
        <dt>检测记录</dt><dd>{usageText(usage, recordsText)}</dd>
      </dl>
      {!restartMessage && <Alert tone="warn" title="切换需要重启软件">正式版确认后自动重启；开发版需要重新运行开发入口。重启后，相机、PLC 与在线检测按目标项目的配置启动。</Alert>}
      {unsaved && <Alert tone="ng" title="不能切换">{unsaved}</Alert>}
      {error && <Alert tone="ng" title="切换未完成">{error}</Alert>}
      {restarting && <Alert tone="info" role="status">{restartMessage || "正在重启…"}</Alert>}
    </Stack>
  </Dialog>;
}
