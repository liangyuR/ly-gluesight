import { desktopCall } from "../../lib/desktop";

export interface LicenseStatus {
  state: "unconfigured" | "unactivated" | "valid" | "expiring" | "expired" | "invalid" | "emergency";
  allowed: boolean;
  message: string;
  expiresAt: string | null;
  warningAt: string | null;
  emergencyUntil: string | null;
  licenseId: string | null;
  deviceFingerprint: string | null;
  configured: boolean;
  emergencyAvailable: boolean;
}

export const licenseApi = {
  status: () => desktopCall<LicenseStatus>("license_status"),
  activate: (licenseId: string, activationSecret: string) => desktopCall<LicenseStatus>("license_activate", { licenseId, activationSecret }),
  sync: () => desktopCall<LicenseStatus>("license_sync"),
  unlock: (password: string, reason: string) => desktopCall<LicenseStatus>("license_emergency_unlock", { password, reason }),
};

export const licenseStateLabel: Record<LicenseStatus["state"], string> = {
  unconfigured: "授权配置未完成", unactivated: "未激活", valid: "授权有效", expiring: "即将到期",
  expired: "授权已到期", invalid: "授权需要处理", emergency: "现场应急放行中",
};

export function licenseDate(value: string | null) {
  return value ? new Date(value).toLocaleString("zh-CN", { timeZone: "Asia/Shanghai", hour12: false }) : "—";
}
