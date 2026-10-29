import { LyFlowEditor, type GraphDoc } from "@lyflow/editor";
import type { FlowTransport } from "./flowTransport";

/** 编辑器单独成块按需加载：three 与 React Flow 只在打开流程调试页时才进来。 */
export default function FlowEditor({ transport, path, onDocChange }: { transport: FlowTransport; path: string; onDocChange: (doc: GraphDoc, dirty: boolean) => void }) {
  return <LyFlowEditor className="wp-flow-editor" transport={transport} graphPath={path} topologyLocked onDocChange={onDocChange} />;
}
