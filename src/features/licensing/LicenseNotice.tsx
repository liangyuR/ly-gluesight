import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { Alert, Button, Dialog } from "qing-steel";
import { desktopAvailable } from "../../lib/desktop";
import { licenseDate, licenseStateLabel } from "./api";
import { useLicense } from "./context";
import "./licensing.css";

export default function LicenseNotice() {
  const { status, error } = useLicense();
  const [reminder, setReminder] = useState(false);
  const seen = useRef("");
  useEffect(() => {
    if (status?.state !== "expiring" || error) { setReminder(false); return; }
    const day = new Date().toLocaleDateString("en-CA", { timeZone: "Asia/Shanghai" });
    const key = `gluesight-license-reminder:${status.licenseId}:${status.expiresAt}`;
    const marker = `${key}:${day}`;
    if (seen.current === marker) return;
    seen.current = marker;
    try {
      if (localStorage.getItem(key) === day) return;
      localStorage.setItem(key, day);
    } catch { /* 提醒记录不可写时，本次运行仍只提醒一次。 */ }
    setReminder(true);
  }, [status, error]);
  if (!desktopAvailable() || (!error && status?.state === "valid")) return null;
  return <div className="license-notice">
    <Alert tone="warn" title={error ? "无法确认授权状态" : status ? licenseStateLabel[status.state] : "正在检查授权"} action={<Link to="/license">授权管理</Link>}>
      {error || status?.message || "业务开始前由本机授权模块完成检查。"}
      {status?.state === "expiring" && <span> 停用时间：{licenseDate(status.expiresAt)}（北京时间）。请续约后联网更新授权。</span>}
      {status?.state === "emergency" && <span> 临时放行至 {licenseDate(status.emergencyUntil)}（北京时间），届时恢复正常授权检查。</span>}
      {(error || (status && !status.allowed)) && <span> 新业务已受限；已有数据仍可查看和导出，已开始的工件按原流程收尾。</span>}
    </Alert>
    <Dialog open={reminder} title="软件授权将在七天内到期" onOpenChange={setReminder} footer={<Button onClick={() => setReminder(false)}>知道了</Button>}>
      <p>停用时间：{licenseDate(status?.expiresAt ?? null)}（北京时间）。后台续约后，请在授权管理中点击“更新授权”领取新证书；未领取仍按本机原期限停用。</p>
      <Link to="/license" onClick={() => setReminder(false)}>前往授权管理</Link>
    </Dialog>
  </div>;
}
