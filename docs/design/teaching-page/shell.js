/* 效果图外壳：侧栏（照搬 src/layout/nav.ts 的分组与文案，配方页高亮）、lucide 风格图标、灰度涂胶工件图与中线叠加 */
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
    minus: '<path d="M5 12h14"/>',
    "rotate-ccw": '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>',
    "chevron-right": '<path d="m9 18 6-6-6-6"/>',
    "chevron-down": '<path d="m6 9 6 6 6-6"/>',
    "arrow-right": '<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>',
    "arrow-up-right": '<path d="M7 7h10v10"/><path d="M7 17 17 7"/>',
    "alert-triangle": '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
    info: '<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
    check: '<path d="M20 6 9 17l-5-5"/>',
    "check-circle": '<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>',
    lock: '<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>',
    brush: '<path d="m9.06 11.9 8.07-8.06a2.85 2.85 0 1 1 4.03 4.03l-8.06 8.08"/><path d="M7.07 14.94c-1.66 0-3 1.35-3 3.02 0 1.33-2.5 1.52-2 2.02 1.08 1.1 2.49 2.02 4 2.02 2.2 0 4-1.8 4-4.04a3.01 3.01 0 0 0-3-3.02z"/>',
    "wand-sparkles": '<path d="m21.64 3.64-1.28-1.28a1.21 1.21 0 0 0-1.72 0L2.36 18.64a1.21 1.21 0 0 0 0 1.72l1.28 1.28a1.2 1.2 0 0 0 1.72 0L21.64 5.36a1.2 1.2 0 0 0 0-1.72"/><path d="m14 7 3 3"/><path d="M5 6v4"/><path d="M19 14v4"/><path d="M10 2v2"/><path d="M7 8H3"/><path d="M21 16h-4"/><path d="M11 3H9"/>',
    "undo-2": '<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"/>',
    "trash-2": '<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/><line x1="10" x2="10" y1="11" y2="17"/><line x1="14" x2="14" y1="11" y2="17"/>',
    eraser: '<path d="m7 21-4.3-4.3c-1-1-1-2.5 0-3.4l9.6-9.6c1-1 2.5-1 3.4 0l5.6 5.6c1 1 1 2.5 0 3.4L13 21"/><path d="M22 21H7"/><path d="m5 11 9 9"/>',
    play: '<polygon points="6 3 20 12 6 21 6 3"/>',
    "delete": '<path d="M10 5a2 2 0 0 0-1.344.519l-6.328 5.74a1 1 0 0 0 0 1.481l6.328 5.741A2 2 0 0 0 10 19h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2z"/><path d="m12 9 6 6"/><path d="m18 9-6 6"/>',
    "refresh-cw": '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>',
    "layout-grid": '<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>',
    "mouse-pointer": '<path d="M12.586 12.586 19 19"/><path d="M3.688 3.037a.497.497 0 0 0-.651.651l6.5 15.999a.501.501 0 0 0 .947-.062l1.569-6.083a2 2 0 0 1 1.448-1.479l6.124-1.579a.5.5 0 0 0 .063-.947z"/>',
    "square-dashed": '<path d="M5 3a2 2 0 0 0-2 2"/><path d="M19 3a2 2 0 0 1 2 2"/><path d="M21 19a2 2 0 0 1-2 2"/><path d="M5 21a2 2 0 0 1-2-2"/><path d="M9 3h1"/><path d="M9 21h1"/><path d="M14 3h1"/><path d="M14 21h1"/><path d="M3 9v1"/><path d="M21 9v1"/><path d="M3 14v1"/><path d="M21 14v1"/>',
    "help-circle": '<circle cx="12" cy="12" r="10"/><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/><path d="M12 17h.01"/>',
    save: '<path d="M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7"/><path d="M7 3v4a1 1 0 0 0 1 1h7"/>'
  };
  function icon(name, size) {
    size = size || 16;
    return '<svg xmlns="http://www.w3.org/2000/svg" width="' + size + '" height="' + size + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + (paths[name] || "") + "</svg>";
  }
  window.icon = icon;

  var nav = [
    ["总览", "layout-dashboard", null],
    ["生产", null, [["在线检测", "scan-eye"], ["历史记录", "history"]]],
    ["工艺", null, [["配方", "book-open", true], ["离线模式", "flask-conical"]]],
    ["设备", null, [["相机与采集", "camera"], ["飞拍标定", "crosshair"], ["PLC 通讯", "cable"], ["通讯日志", "scroll-text"]]]
  ];
  function navItem(label, ic, active) {
    return '<a class="nav-item' + (active ? " active" : "") + '" href="#"' + (active ? ' aria-current="page"' : "") + ">" + icon(ic, 17) + '<span class="nav-label">' + label + "</span></a>";
  }
  function sidebar() {
    var html = '<aside class="sidebar"><div class="sidebar-brand"><div class="brand-logo">' + icon("camera", 18) + '</div><div class="brand-text"><strong>GlueSight</strong><span>胶路智检</span></div></div><nav class="sidebar-nav" aria-label="操作导航">';
    nav.forEach(function (e) {
      if (e[2]) html += '<div class="nav-group"><div class="nav-group-label">' + e[0] + "</div>" + e[2].map(function (c) { return navItem(c[0], c[1], c[2]); }).join("") + "</div>";
      else html += navItem(e[0], e[1], false);
    });
    html += '</nav><div class="sidebar-footer">' + navItem("系统设置", "settings", false) + '<div class="status-list"><span class="engine-status"><span class="dot ok"></span><span class="status-text">PLC · 已连接</span></span><span class="engine-status"><span class="dot ok"></span><span class="status-text">LyFlow · 就绪</span></span></div><div class="sidebar-actions"><button class="qs-icon-btn" aria-label="折叠侧边栏">' + icon("panel-left-close", 18) + "</button></div></aside>";
    return html;
  }

  /* 灰度涂胶工件近景：CSS/SVG 占位，不外链图片 */
  var curves = {
    1: "M -10 150 C 60 120, 120 70, 200 86 S 320 130, 340 60",
    2: "M -10 60 C 70 70, 90 150, 170 140 S 270 80, 340 110",
    3: "M -10 110 C 80 100, 140 100, 180 70 S 260 40, 340 150"
  };
  function bead(variant) {
    var id = "g" + Math.random().toString(36).slice(2, 8);
    var d = curves[variant] || curves[1];
    return '<svg viewBox="0 0 320 200" preserveAspectRatio="xMidYMid slice" role="img" aria-label="涂胶工件近景（灰度）">' +
      '<defs><filter id="' + id + 'n" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" seed="' + (variant * 7) + '" stitchTiles="stitch"/><feColorMatrix type="matrix" values="0 0 0 0 0.5 0 0 0 0 0.5 0 0 0 0 0.5 0 0 0 0.35 0"/></filter>' +
      '<linearGradient id="' + id + 'bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#4a4a4a"/><stop offset="1" stop-color="#2b2b2b"/></linearGradient>' +
      '<linearGradient id="' + id + 'bead" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#6e6e6e"/><stop offset="0.45" stop-color="#dcdcdc"/><stop offset="1" stop-color="#555"/></linearGradient>' +
      '<radialGradient id="' + id + 'vig" cx="0.5" cy="0.5" r="0.75"><stop offset="0.6" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity="0.55"/></radialGradient></defs>' +
      '<rect width="320" height="200" fill="url(#' + id + 'bg)"/><rect width="320" height="200" filter="url(#' + id + 'n)" opacity="0.6"/>' +
      '<g stroke="#3a3a3a" stroke-width="1" opacity="0.7"><line x1="0" y1="178" x2="320" y2="170"/><line x1="0" y1="22" x2="320" y2="30"/><line x1="250" y1="0" x2="262" y2="200"/></g>' +
      '<path d="' + d + '" fill="none" stroke="#202020" stroke-width="30" stroke-linecap="round" opacity="0.55"/>' +
      '<path d="' + d + '" fill="none" stroke="url(#' + id + 'bead)" stroke-width="22" stroke-linecap="round"/>' +
      '<path d="' + d + '" fill="none" stroke="#f2f2f2" stroke-width="4" stroke-linecap="round" opacity="0.8" transform="translate(0,-5)"/>' +
      '<rect width="320" height="200" fill="url(#' + id + 'vig)"/></svg>';
  }
  window.bead = bead;

  /* 中线叠加：沿同一条胶路曲线采样 n 个编辑点；mode=draft（待试测，青色）/ trial（试测结果：各站绿色胶宽横线，个别红色候选） */
  function overlay(variant, mode, n) {
    var ns = "http://www.w3.org/2000/svg";
    var svg = document.createElementNS(ns, "svg");
    svg.setAttribute("viewBox", "0 0 320 200"); svg.setAttribute("preserveAspectRatio", "xMidYMid slice"); svg.setAttribute("aria-hidden", "true");
    var path = document.createElementNS(ns, "path"); path.setAttribute("d", curves[variant] || curves[1]);
    svg.appendChild(path);
    var total = path.getTotalLength(), pts = [];
    for (var i = 0; i <= n; i++) {
      var p = path.getPointAtLength(total * (0.07 + 0.86 * i / n)), q = path.getPointAtLength(total * (0.07 + 0.86 * i / n) + 0.5);
      pts.push([p.x, p.y, Math.atan2(q.y - p.y, q.x - p.x)]);
    }
    var html = '<polyline fill="none" stroke="var(--primary)" stroke-width="1.6" stroke-dasharray="' + (mode === "draft" ? "4 3" : "none") + '" opacity="0.9" points="' + pts.map(function (p) { return p[0].toFixed(1) + "," + p[1].toFixed(1); }).join(" ") + '"/>';
    pts.forEach(function (p, i) {
      if (mode === "trial") {
        var bad = i === Math.floor(n * 0.62), half = 11, nx = -Math.sin(p[2]) * half, ny = Math.cos(p[2]) * half, c = bad ? "var(--ng)" : "var(--ok)";
        html += '<line x1="' + (p[0] - nx) + '" y1="' + (p[1] - ny) + '" x2="' + (p[0] + nx) + '" y2="' + (p[1] + ny) + '" stroke="' + c + '" stroke-width="1.8"/>';
        html += '<circle cx="' + p[0] + '" cy="' + p[1] + '" r="2.4" fill="' + c + '"/>';
        if (i % 4 === 0 && p[0] < 270 && p[0] > 30) html += '<text x="' + (p[0] + nx + 3) + '" y="' + (p[1] + ny + 3) + '" font-size="7" font-family="var(--font-mono)" fill="' + c + '">' + (bad ? "3.91 mm 候选" : (3.42 + (i % 5) * 0.03).toFixed(2) + " mm") + "</text>";
      } else {
        html += '<circle cx="' + p[0] + '" cy="' + p[1] + '" r="' + (i === 0 ? 4 : 2.6) + '" fill="' + (i === 0 ? "none" : "var(--primary)") + '" stroke="' + (i === 0 ? "var(--ok)" : "var(--surface-lowest)") + '" stroke-width="1.2"/>';
      }
    });
    if (mode === "failed") { var f = pts[Math.floor(n * 0.55)]; html = '<path d="' + (curves[variant] || curves[1]) + '" fill="none" stroke="var(--primary)" stroke-width="' + 26 + '" stroke-linecap="round" opacity="0.18"/>' + '<circle cx="' + f[0] + '" cy="' + f[1] + '" r="7" fill="none" stroke="var(--ng)" stroke-width="1.6"/><line x1="' + (f[0]-4) + '" y1="' + (f[1]-4) + '" x2="' + (f[0]+4) + '" y2="' + (f[1]+4) + '" stroke="var(--ng)" stroke-width="1.6"/><line x1="' + (f[0]+4) + '" y1="' + (f[1]-4) + '" x2="' + (f[0]-4) + '" y2="' + (f[1]+4) + '" stroke="var(--ng)" stroke-width="1.6"/>'; }
    if (mode === "draft") html += '<circle cx="' + pts[6][0] + '" cy="' + pts[6][1] + '" r="5" fill="none" stroke="var(--warn)" stroke-width="1.5"/>';
    svg.innerHTML = html;
    return svg;
  }

  document.addEventListener("DOMContentLoaded", function () {
    var shell = document.querySelector(".app-shell");
    if (shell && !shell.querySelector(".sidebar")) shell.insertAdjacentHTML("afterbegin", sidebar());
    document.querySelectorAll("i[data-icon]").forEach(function (el) { el.outerHTML = icon(el.getAttribute("data-icon"), el.getAttribute("data-size") || 16); });
    document.querySelectorAll("[data-bead]").forEach(function (el) {
      el.insertAdjacentHTML("afterbegin", bead(Number(el.getAttribute("data-bead")) || 1));
      var mode = el.getAttribute("data-overlay");
      if (mode) el.appendChild(overlay(Number(el.getAttribute("data-bead")) || 1, mode, Number(el.getAttribute("data-points")) || 14));
    });
    var scroll = document.body.getAttribute("data-scroll");
    if (scroll) { var body = document.querySelector(".main-body"); if (body) body.scrollTop = scroll === "bottom" ? body.scrollHeight : Number(scroll); }
  });
})();
