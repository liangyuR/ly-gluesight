import { PlcLogPage, PlcPage } from "../features/plc";
import { inspectionTagPresets } from "../business/plcTags";
import { ReturnButton } from "../features/workspace/returnNavigation";

export function PlcSettingsPage() {
  return (
    <div className="stack">
      <ReturnButton />
      <PlcPage tagPresets={inspectionTagPresets} />
    </div>
  );
}

export function PlcLogsPage() {
  return <PlcLogPage />;
}
