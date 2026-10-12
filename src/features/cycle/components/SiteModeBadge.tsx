import { Link } from "react-router-dom";
import { useSiteStatus } from "../api";
import { siteModeText } from "../offline";

export default function SiteModeBadge() {
  const status = useSiteStatus();
  if (!status) return null;
  const text = siteModeText(status);
  return (
    <Link to="/offline" className={`site-badge${status.mode === "offline" ? " offline" : ""}`} aria-label={`站点模式：${text}`} title="打开离线控制台">
      {text}
    </Link>
  );
}
