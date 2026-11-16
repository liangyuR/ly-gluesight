import { useState, type FormEvent } from "react";
import { Alert, Button, Card, TextField } from "qing-steel";
import { desktopAvailable } from "../lib/desktop";
import { errorText } from "../lib/errors";
import { licenseApi, licenseDate, licenseStateLabel } from "../features/licensing/api";
import { useLicense } from "../features/licensing/context";
import "../features/licensing/licensing.css";

export default function LicensePage() {
  const { status, error, busy, refresh, execute } = useLicense();
  const [licenseId, setLicenseId] = useState("");
  const [secret, setSecret] = useState("");
  const [password, setPassword] = useState("");
  const [reason, setReason] = useState("");
  const [failure, setFailure] = useState("");
  const [success, setSuccess] = useState("");
  const available = desktopAvailable();
  const blocked = !available || busy;
  const run = async (action: Parameters<typeof execute>[0], message: string) => {
    setFailure(""); setSuccess("");
    try { await execute(action); setSuccess(message); }
    catch (err) { setFailure(errorText(err)); }
  };
  const activate = (event: FormEvent) => {
    event.preventDefault();
    const value = secret;
    setSecret("");
    void run(() => licenseApi.activate(licenseId.trim(), value), "授权已验证并保存，请核对本机停用时间。");
  };
  const unlock = (event: FormEvent) => {
    event.preventDefault();
    const value = password;
    setPassword("");
    void run(() => licenseApi.unlock(value, reason.trim()), "现场应急放行已生效，操作已记录。请在临时期限内处理正式授权。");
  };
  return <div className="license-page">
    <h1>软件授权</h1>
    <p>首次联网激活后，可离线使用至合同到期。后台续约后需主动联网更新授权，未领取新证书仍按本机原停用时间执行。</p>
    {!available && <Alert tone="warn">浏览器查看模式无法读取设备身份或执行授权操作，请使用桌面应用。</Alert>}
    {(error || failure) && <Alert tone="ng" title="授权操作未完成">{failure || error}</Alert>}
    {success && <Alert tone="ok">{success}</Alert>}
    <Card title={<h2>本机授权状态</h2>}>
      <dl>
        <dt>状态</dt><dd>{status ? licenseStateLabel[status.state] : available ? "检查中" : "仅可查看页面"}</dd>
        <dt>授权编号</dt><dd>{status?.licenseId || "—"}</dd>
        <dt>停用时间</dt><dd>{licenseDate(status?.expiresAt ?? null)}（北京时间）</dd>
        <dt>提前七天提醒</dt><dd>{licenseDate(status?.warningAt ?? null)}（北京时间）</dd>
        <dt>设备指纹摘要</dt><dd>{status?.deviceFingerprint || "—"}</dd>
        {status?.emergencyUntil && <><dt>应急放行截止</dt><dd>{licenseDate(status.emergencyUntil)}（北京时间）</dd></>}
      </dl>
      <p>{status?.message}</p>
      <div className="license-actions">
        <Button disabled={blocked} onClick={() => void refresh()}>刷新状态</Button>
        <Button variant="filled" disabled={blocked || !status?.licenseId || !status.configured} onClick={() => void run(licenseApi.sync, "授权已更新并保存，请核对新的停用时间。")}>更新授权</Button>
        {busy && <span role="status">正在处理，请稍候…</span>}
      </div>
    </Card>
    <Card title={<h2>联网激活</h2>}>
      <form onSubmit={activate}>
        <TextField label="授权 UUID" value={licenseId} required maxLength={36} disabled={blocked} onChange={e => setLicenseId(e.target.value)} autoComplete="off" />
        <TextField label="激活秘密" type="password" value={secret} required maxLength={512} disabled={blocked} onChange={e => setSecret(e.target.value)} autoComplete="off" />
        <p>请使用管理员交付的授权 UUID 和激活秘密。每份授权只绑定一台设备；激活秘密不会保存在页面中。</p>
        <div><Button type="submit" variant="filled" disabled={blocked || !status?.configured || !licenseId.trim() || !secret}>激活授权</Button></div>
      </form>
    </Card>
    <Card title={<h2>现场应急解锁</h2>}>
      <form onSubmit={unlock}>
        <p>仅由维护人员处理现场授权故障。正确密码每次临时放行 72 小时，界面持续提示并记录使用原因；不会改变正式许可证期限。</p>
        {!status?.emergencyAvailable && <Alert tone="warn">此安装版本未配置应急密码，请联系维护人员。</Alert>}
        <TextField label="应急密码" type="password" autoComplete="off" value={password} required maxLength={256} disabled={blocked || !status?.emergencyAvailable} onChange={e => setPassword(e.target.value)} />
        <TextField label="使用原因" value={reason} required maxLength={500} disabled={blocked || !status?.emergencyAvailable} onChange={e => setReason(e.target.value)} />
        <div><Button type="submit" disabled={blocked || !status?.emergencyAvailable || !password || !reason.trim()}>临时解锁 72 小时</Button></div>
      </form>
    </Card>
  </div>;
}
