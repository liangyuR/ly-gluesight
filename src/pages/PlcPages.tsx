import { PlcLogPage, PlcPage } from "../features/plc";
import { OfflineManagedNotice, useSiteStatus } from "../features/cycle";
import { inspectionTagPresets, teachingTagPresets } from "../business/plcTags";
import { ReturnButton } from "../features/workspace/returnNavigation";

export function PlcSettingsPage() {
  const offline = useSiteStatus()?.mode === "offline";
  return (
    <div className="stack">
      <ReturnButton />
      {offline && <OfflineManagedNotice>PLC 临时连接进程内 S7 服务，离线期间不能修改 PLC 配置。</OfflineManagedNotice>}
      <PlcPage key={offline ? "offline" : "real"} readOnly={offline} tagPresets={[...inspectionTagPresets, ...teachingTagPresets]} />
    </div>
  );
}

export function PlcLogsPage() {
  return <PlcLogPage />;
}
