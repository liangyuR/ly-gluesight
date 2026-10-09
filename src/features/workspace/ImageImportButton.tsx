import { useEffect, useRef, useState } from "react";
import { Upload } from "lucide-react";

export function validateImageFile(file: File): void {
  if (!/\.(png|jpe?g|pgm|bmp|tiff?)$/i.test(file.name)) throw new Error("请选择 PNG、JPEG、PGM、BMP 或 TIFF 原图");
  if (!file.size || file.size > 15_000_000) throw new Error("原图不得为空，单图不得超过 15 MB");
}

export async function readImageFile(file: File): Promise<number[]> {
  validateImageFile(file);
  return Array.from(new Uint8Array(await file.arrayBuffer()));
}

export default function ImageImportButton({ scope, disabled, onImport, onError, onReadingChange, label = "导入离线原图" }: {
  scope: string; disabled?: boolean; onImport: (bytes: number[]) => Promise<unknown>; onError: (error: string) => void; onReadingChange?: (reading:boolean)=>void; label?: string;
}) {
  const input = useRef<HTMLInputElement>(null);
  const pending = useRef(false);
  const serial=useRef(0),reported=useRef(false);
  const current = useRef({ scope, alive: true, onReadingChange });
  current.current.scope = scope;current.current.onReadingChange=onReadingChange;
  const [reading, setReading] = useState(false);
  const report=(value:boolean)=>{if(reported.current!==value){reported.current=value;current.current.onReadingChange?.(value);}};
  useEffect(() => { current.current.alive = true; return () => { current.current.alive = false;serial.current++;report(false); }; }, []);
  useEffect(()=>{serial.current++;pending.current=false;setReading(false);report(false);},[scope]);
  const importFile = async (file: File) => {
    if (pending.current || disabled) return;
    const request=++serial.current;
    pending.current = true; setReading(true);report(true);
    const valid = () => current.current.alive && current.current.scope === scope&&request===serial.current;
    try { const bytes = await readImageFile(file); if (valid()) await onImport(bytes); }
    catch (e) { if (valid()) onError(String(e)); }
    finally { if(request===serial.current){pending.current = false;report(false);if (current.current.alive) setReading(false);} }
  };
  return <>
    <button type="button" className="btn" disabled={disabled || reading} onClick={() => input.current?.click()}><Upload size={15}/>{reading ? "正在导入…" : label}</button>
    <input ref={input} aria-label={label} type="file" hidden accept=".png,.jpg,.jpeg,.pgm,.bmp,.tif,.tiff" disabled={disabled || reading}
      onChange={e => { const file = e.target.files?.[0]; e.target.value = ""; if (file) void importFile(file); }}/>
  </>;
}
