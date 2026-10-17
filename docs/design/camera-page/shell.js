/* 效果图外壳：侧栏（照搬 src/layout/nav.ts 的分组与文案）、lucide 风格图标、灰度涂胶工件缩略图 */
(function () {
  var paths = {
    "layout-dashboard": '<rect width="7" height="9" x="3" y="3" rx="1"/><rect width="7" height="5" x="14" y="3" rx="1"/><rect width="7" height="9" x="14" y="12" rx="1"/><rect width="7" height="5" x="3" y="16" rx="1"/>',
    "scan-eye": '<path d="M3 7V5a2 2 0 0 1 2-2h2"/><path d="M17 3h2a2 2 0 0 1 2 2v2"/><path d="M21 17v2a2 2 0 0 1-2 2h-2"/><path d="M7 21H5a2 2 0 0 1-2-2v-2"/><circle cx="12" cy="12" r="1"/><path d="M18.944 12.33a1 1 0 0 0 0-.66 7.5 7.5 0 0 0-13.888 0 1 1 0 0 0 0 .66 7.5 7.5 0 0 0 13.888 0"/>',
    history: '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/>',
    "book-open": '<path d="M12 7v14"/><path d="M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z"/>',
    "flask-conical": '<path d="M10 2v7.527a2 2 0 0 1-.211.896L4.72 20.55a1 1 0 0 0 .9 1.45h12.76a1 1 0 0 0 .9-1.45l-5.069-10.127A2 2 0 0 1 14 9.527V2"/><path d="M8.5 2h7"/><path d="M7 16h10"/>',
    camera: '<path d="M14.5 4h-5L7 7H4a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-3l-2.5-3z"/><circle cx="12" cy="13" r="3"/>',
    crosshair: '<circle cx="12" cy="12" r="10"/><line x1="22" x2="18" y1="12" y2="12"/><line x1="6" x2="2" y1="12" y2="12"/><line x1="12" x2="12" y1="6" y2="2"/><line x1="12" x2="12" y1="22" y2="18"/>',
    cable: '<path d="M17 21v-2a1 1 0 0 1-1-1v-1a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v1a1 1 0 0 1-1 1"/><path d="M19 15V6.5a1 1 0 0 0-7 0v11a1 1 0 0 1-7 0V9"/><path d="M21 21v-2h-4"/><path d="M3 5h4V3"/><path d="M7 5a1 1 0 0 1 1 1v1a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a1 1 0 0 1 1-1V3"/>',
    "scroll-text": '<path d="M15 12h-5"/><path d="M15 8h-5"/><path d="M19 17V5a2 2 0 0 0-2-2H4"/><path d="M8 21h12a2 2 0 0 0 2-2v-1a1 1 0 0 0-1-1H11a1 1 0 0 0-1 1v1a2 2 0 1 1-4 0V5a2 2 0 1 0-4 0v2a1 1 0 0 0 1 1h3"/>',
    settings: '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>',
    "panel-left-close": '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/><path d="m16 15-3-3 3-3"/>',
    plus: '<path d="M5 12h14"/><path d="M12 5v14"/>',
    "refresh-cw": '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>',
    zap: '<path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"/>',
    "chevron-right": '<path d="m9 18 6-6-6-6"/>',
    "chevron-left": '<path d="m15 18-6-6 6-6"/>',
    "chevron-down": '<path d="m6 9 6 6 6-6"/>',
    "alert-triangle": '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
    info: '<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
    check: '<path d="M20 6 9 17l-5-5"/>',
    "check-circle": '<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>',
    x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
    minus: '<path d="M5 12h14"/>',
    unplug: '<path d="m19 5 3-3"/><path d="m2 22 3-3"/><path d="M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z"/><path d="M7.5 13.5 10 11"/><path d="M10.5 16.5 13 14"/><path d="m12 6 6 6 2.3-2.3a2.4 2.4 0 0 0 0-3.4l-2.6-2.6a2.4 2.4 0 0 0-3.4 0Z"/>',
    "more-horizontal": '<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>',
    "external-link": '<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
    lock: '<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>',
    "trash-2": '<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/><line x1="10" x2="10" y1="11" y2="17"/><line x1="14" x2="14" y1="11" y2="17"/>',
    copy: '<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>',
    clock: '<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>',
    pause: '<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>',
    link: '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>',
    "link-off": '<path d="M9 17H7A5 5 0 0 1 7 7"/><path d="M15 7h2a5 5 0 0 1 4 8"/><line x1="8" x2="12" y1="12" y2="12"/><line x1="2" x2="22" y1="2" y2="22"/>',
    radio: '<path d="M4.9 19.1C1 15.2 1 8.8 4.9 4.9"/><path d="M7.8 16.2c-2.3-2.3-2.3-6.1 0-8.5"/><circle cx="12" cy="12" r="2"/><path d="M16.2 7.8c2.3 2.3 2.3 6.1 0 8.5"/><path d="M19.1 4.9C23 8.8 23 15.1 19.1 19"/>',
    activity: '<path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"/>',
    database: '<ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5V19A9 3 0 0 0 21 19V5"/><path d="M3 12A9 3 0 0 0 21 12"/>',
    "arrow-right": '<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>',
    "sliders": '<line x1="4" x2="4" y1="21" y2="14"/><line x1="4" x2="4" y1="10" y2="3"/><line x1="12" x2="12" y1="21" y2="12"/><line x1="12" x2="12" y1="8" y2="3"/><line x1="20" x2="20" y1="21" y2="16"/><line x1="20" x2="20" y1="12" y2="3"/><line x1="2" x2="6" y1="14" y2="14"/><line x1="10" x2="14" y1="8" y2="8"/><line x1="18" x2="22" y1="16" y2="16"/>'
  };
  function icon(name, size) {
    size = size || 16;
    return '<svg xmlns="http://www.w3.org/2000/svg" width="' + size + '" height="' + size + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + (paths[name] || "") + "</svg>";
  }
  window.icon = icon;

  var nav = [
    ["总览", "layout-dashboard", null],
    ["生产", null, [["在线检测", "scan-eye"], ["历史记录", "history"]]],
    ["工艺", null, [["配方", "book-open"], ["离线模式", "flask-conical"]]],
    ["设备", null, [["相机与采集", "camera", true], ["飞拍标定", "crosshair"], ["PLC 通讯", "cable"], ["通讯日志", "scroll-text"]]]
  ];
  function navItem(label, ic, active) {
    return '<a class="nav-item' + (active ? " active" : "") + '" href="#"' + (active ? ' aria-current="page"' : "") + ">" + icon(ic, 17) + '<span class="nav-label">' + label + "</span></a>";
  }
  function sidebar() {
    var html = '<aside class="sidebar"><div class="sidebar-brand"><div class="brand-logo">' + icon("camera", 18) + '</div><div class="brand-text"><strong>GlueSight</strong><span>胶路智检</span></div></div><nav class="sidebar-nav" aria-label="操作导航">';
    nav.forEach(function (e) {
      if (e[2]) {
        html += '<div class="nav-group"><div class="nav-group-label">' + e[0] + "</div>" + e[2].map(function (c) { return navItem(c[0], c[1], c[2]); }).join("") + "</div>";
      } else html += navItem(e[0], e[1], false);
    });
    var engine = document.body.getAttribute("data-engine") || "LyFlow · 就绪";
    var plc = document.body.getAttribute("data-plc") || "PLC · 已连接";
    html += '</nav><div class="sidebar-footer">' + navItem("系统设置", "settings", false) + '<div class="status-list"><span class="engine-status"><span class="dot ok"></span><span class="status-text">' + plc + '</span></span><span class="engine-status"><span class="dot ok"></span><span class="status-text">' + engine + '</span></span></div><div class="sidebar-actions"><button class="icon-btn" aria-label="折叠侧边栏">' + icon("panel-left-close", 18) + "</button></div></div></aside>";
    return html;
  }

  /* 灰度涂胶工件近景：CSS/SVG 占位，不外链图片。variant 决定胶路走向，让三台相机的图不一样 */
  function bead(variant, opts) {
    opts = opts || {};
    var id = "g" + Math.random().toString(36).slice(2, 8);
    var curves = {
      1: "M -10 150 C 60 120, 120 70, 200 86 S 320 130, 340 60",
      2: "M -10 60 C 70 70, 90 150, 170 140 S 270 80, 340 110",
      3: "M -10 110 C 80 100, 140 100, 180 70 S 260 40, 340 150"
    };
    var d = curves[variant] || curves[1];
    var ngMark = opts.ng ? '<circle cx="190" cy="82" r="20" fill="none" stroke="#9a9a9a" stroke-width="1.5" stroke-dasharray="3 3"/>' : "";
    return '<svg viewBox="0 0 320 200" preserveAspectRatio="xMidYMid slice" role="img" aria-label="涂胶工件近景（灰度）">' +
      '<defs><filter id="' + id + 'n" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" seed="' + (variant * 7) + '" stitchTiles="stitch"/><feColorMatrix type="matrix" values="0 0 0 0 0.5 0 0 0 0 0.5 0 0 0 0 0.5 0 0 0 0.35 0"/></filter>' +
      '<linearGradient id="' + id + 'bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#4a4a4a"/><stop offset="1" stop-color="#2b2b2b"/></linearGradient>' +
      '<linearGradient id="' + id + 'bead" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#6e6e6e"/><stop offset="0.45" stop-color="#dcdcdc"/><stop offset="1" stop-color="#555"/></linearGradient>' +
      '<radialGradient id="' + id + 'vig" cx="0.5" cy="0.5" r="0.75"><stop offset="0.6" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity="0.55"/></radialGradient></defs>' +
      '<rect width="320" height="200" fill="url(#' + id + 'bg)"/>' +
      '<rect width="320" height="200" filter="url(#' + id + 'n)" opacity="0.6"/>' +
      '<g stroke="#3a3a3a" stroke-width="1" opacity="0.7"><line x1="0" y1="178" x2="320" y2="170"/><line x1="0" y1="22" x2="320" y2="30"/><line x1="250" y1="0" x2="262" y2="200"/></g>' +
      '<path d="' + d + '" fill="none" stroke="#202020" stroke-width="30" stroke-linecap="round" opacity="0.55"/>' +
      '<path d="' + d + '" fill="none" stroke="url(#' + id + 'bead)" stroke-width="22" stroke-linecap="round"/>' +
      '<path d="' + d + '" fill="none" stroke="#f2f2f2" stroke-width="4" stroke-linecap="round" opacity="0.8" transform="translate(0,-5)"/>' +
      ngMark +
      '<rect width="320" height="200" fill="url(#' + id + 'vig)"/>' +
      "</svg>";
  }
  window.bead = bead;

  document.addEventListener("DOMContentLoaded", function () {
    var shell = document.querySelector(".app-shell");
    if (shell && !shell.querySelector(".sidebar")) shell.insertAdjacentHTML("afterbegin", sidebar());
    document.querySelectorAll("i[data-icon]").forEach(function (el) {
      var size = el.getAttribute("data-size") || 16;
      el.outerHTML = icon(el.getAttribute("data-icon"), size);
    });
    document.querySelectorAll("[data-bead]").forEach(function (el) {
      el.insertAdjacentHTML("afterbegin", bead(Number(el.getAttribute("data-bead")) || 1, { ng: el.hasAttribute("data-ng") }));
    });
    var scroll = document.body.getAttribute("data-scroll");
    if (scroll) {
      var body = document.querySelector(".main-body");
      if (body) body.scrollTop = scroll === "bottom" ? body.scrollHeight : Number(scroll);
    }
  });
})();
