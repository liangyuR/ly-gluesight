# -*- coding: utf-8 -*-
"""生成 v3 各状态的静态 HTML（python build.py）。结构与高度方案见 README「v3」。"""
import os
HERE = os.path.dirname(os.path.abspath(__file__))

GROUPS = [("S001","ok","已示教"),("S002","ok","已示教"),("S003","ok","已示教"),("S004","ok","已示教"),("S005","warn","待试测"),("S006","warn","未配置"),
          ("S007","off","待选图"),("S008","off","待选图"),("S009","off","待选图"),("S010","off","待选图"),("S011","off","待选图"),("S012","off","待选图")]

def ic(name, size=16): return f'<i data-icon="{name}" data-size="{size}"></i>'

def groups_html(selected, override=None, disabled=False, tooltip_on=None):
    out = []
    for gid, tone, status in GROUPS:
        if override and gid in override: tone, status = override[gid]
        sel = " selected" if gid == selected else ""
        dis = " disabled" if disabled and gid != selected else ""
        btn = f'<button class="wp-group-item{sel}"{dis}{" aria-pressed=\"true\"" if sel else ""}><i class="dot {tone}"></i><strong>{gid}</strong><span>{status}</span></button>'
        if tooltip_on == gid:
            # 禁用按钮收不到 pointer 事件：实现时外包 <span tabIndex=0> 再挂 Tooltip
            btn = f'<span class="rel block" tabindex="0">{btn}<span class="qs-tooltip below start" style="left:0;top:38px">有未试测的修改，先试测或清空后再切组</span></span>'
        out.append(btn)
    return "\n".join(out)

def picker(group, disabled=False, production=False, info_open=False, selected=0, multi=False, no_link=False):
    dis = " disabled" if disabled else ""
    thumbs = []
    for i in range(3):
        is_sel = (i == selected) if selected is not None else False
        is_multi = multi and i in (0, 1)
        cls = " selected" if is_sel and not multi else (" multi" if is_multi else "")
        cap = "当前检测图" if is_sel and not multi else ("已选 · 需重选" if is_multi else "选用此图")
        thumbs.append(f'<button class="wp-view-option{cls}"{dis}{" aria-pressed=\"true\"" if is_sel else ""}><span class="wp-view-thumbnail" data-bead="{i+1}"></span><span class="wp-view-caption"><strong>cam{i+1}<span class="view-no"> · 图 1</span></strong><span class="small">{cap}</span></span></button>')
    pop = ""
    if info_open:
        pop = ('<div class="qs-popover"><h3>当前检测图 · S005 · cam1 · 图 1</h3>'
               '<div class="wp-kv"><span>图像</span><strong class="mono">img_0f3a2c · 2448 × 2048 px</strong></div><div class="wp-kv"><span>取样来源</span><strong>整圈采集 · 2026/10/18 14:02</strong></div>'
               '<div class="wp-kv"><span>曝光 / 增益</span><strong>1200 μs / 6 dB</strong></div><div class="wp-kv"><span>原图更新</span><strong>仅通过整圈重新采集</strong></div>'
               '<p class="c-warn">采集时曝光 1200 µs / 增益 6 dB，与配方当前值（1500 µs / 6 dB）不同；样本仍可用，需要时整圈重采。</p>'
               '<p>中线保存进候选配方，已发布版本不变。用于生产前仍需完成标定、代表性样本验证与发布。</p></div>')
    side = f'<button class="qs-icon-btn sm" aria-label="图像采集信息" aria-expanded="{"true" if info_open else "false"}">{ic("info",15)}</button>'
    if production:
        side += f'<span class="wp-text-link locked" aria-disabled="true" title="工件正在检测，重采需等待工件结束">{ic("lock",12)}重新采集</span>'
    elif not no_link:
        side += '<a class="wp-text-link" href="#">重新采集</a>'
    warn = '<p class="wp-picker-warn c-warn" role="alert">本组原先选择了多张图，请重新选择一张作为检测图</p>' if multi else ""
    return f'''<section class="wp-picker" aria-label="本组检测图选择">
              <h2>{group}<small>检测图</small></h2>
              <div class="wp-group-images" role="group" aria-label="本组图像">{"".join(thumbs)}</div>
              <div class="wp-picker-side">{side}</div>
            </section>{warn}{pop}'''

SEG = [("edit","mouse-pointer","编辑点","编辑"),("brush","brush","沿胶路刷选","刷选"),("box","square-dashed","矩形框选","框选")]
def toolbar(mode="edit", primary=None, extract_disabled=False, stroke_disabled=True):
    seg = "".join(f'<button{" aria-pressed=\"true\"" if mode == m else ""} title="{label}">{ic(icon,14)}<span class="seg-label">{label}</span><span class="seg-short">{short}</span></button>' for m, icon, label, short in SEG)
    def variant(k): return "filled" if primary == k else ""
    return f'''<div class="wp-image-tools">
                <div class="wp-canvas-tools" role="group" aria-label="示教工具">
                  <div class="qs-seg" aria-label="画布模式">{seg}</div>
                  <button class="qs-btn sm with-leading {variant("extract")}"{" disabled" if extract_disabled else ""}>{ic("wand-sparkles",14)}自动提取中线</button>
                  <button class="qs-btn sm {variant("stroke")}"{" disabled" if stroke_disabled else ""}>笔划直接生成中线</button>
                  <span class="qs-divider v"></span>
                  <button class="qs-icon-btn" title="删除末点" aria-label="删除末点">{ic("delete",15)}</button>
                  <button class="qs-icon-btn" title="删除选中点" aria-label="删除选中点">{ic("trash-2",15)}</button>
                  <button class="qs-icon-btn" title="清空中线" aria-label="清空中线">{ic("eraser",15)}</button>
                </div>
              </div>'''

def params(mode="edit", hint="", hint_tone=""):
    brush = '<span class="wp-inline-num">笔刷宽度<span class="qs-input sm num"><input value="120"><span class="unit">px</span></span><span class="qs-slider" style="--v:39%"><i></i><b></b></span></span>' if mode == "brush" else ""
    h = ""
    if hint:
        h = f'<p class="wp-canvas-hint warn" role="status">{ic("alert-triangle",14)}<span>{hint}</span></p>' if hint_tone == "warn" else f'<p class="wp-canvas-hint" role="status">{hint}</p>'
    return f'''<div class="wp-canvas-params">
                {brush}
                <span class="wp-inline-num">中线点间距<span class="qs-input sm num"><input value="20"><span class="unit">px</span></span><button class="qs-btn sm text">应用到当前中线</button></span>
                {h}
              </div>'''

def canvas(overlay="draft", points=14, mode="edit", group="S005", trial=None, band=False, footer_text="点击图像加中线点（绿圈为起点），拖动点可调整"):
    """trial: None=未试测 | ("ok","合格","97.5%","212 ms") | ("warn","试测已过期",...)"""
    ov = f' data-overlay="{overlay}" data-points="{points}"' if overlay else ""
    if trial is None:
        left = f'<span class="wp-trial-inline"><span class="qs-badge neutral"><span class="dot"></span>未试测</span><span class="muted">中线 <b>{points+1}</b> 点 · <b>182.4</b> mm</span></span>'
    else:
        tone, label, cov, ms = trial
        extra = f'<span class="muted">量成 <b>{cov}</b><span class="ms"> · <b>{ms}</b></span> · <b>{points+1}</b> 点 · <b>182.4</b> mm · <b>{points+1}</b> 站</span>' if cov else f'<span class="muted">中线 <b>{points+1}</b> 点 · <b>182.4</b> mm</span>'
        left = f'<span class="wp-trial-inline"><span class="qs-badge {tone}"><span class="dot"></span>{label}</span>{extra}</span>'
    handles = '<button class="qs-chip selected">点柄</button>' if mode == "edit" else ""
    return f'''<div class="wp-gray-image" data-bead="1"{ov}></div>
              <div class="wp-image-footer">{left}
                <div class="wp-zoom"><button class="qs-icon-btn sm" aria-label="缩小">{ic("minus",15)}</button><span class="mono">100%</span><button class="qs-icon-btn sm" aria-label="放大">{ic("plus",15)}</button><button class="qs-icon-btn sm" aria-label="适应窗口">{ic("rotate-ccw",15)}</button>
                <button class="qs-chip selected">中线</button><button class="qs-chip{" selected" if band else ""}">搜索带</button>{handles}</div>
              </div>'''

def trial_details(open_=False):
    body = '''<div class="body"><span class="wp-trial-legend muted"><i class="ok"></i>有效胶宽<i class="ng"></i>候选胶宽未通过（仅供参考，不影响判定）</span><p class="hint muted">绿色横线为有效胶宽，红色横线为未通过的候选胶宽，仅供参考，不改变 NG 判定；无候选胶边时保留红点。</p>
                <table class="qs-table"><thead><tr><th class="num">测量站</th><th>状态</th><th class="num">偏移 mm</th><th class="num">胶宽 mm</th></tr></thead>
                <tbody><tr><td class="num">1</td><td class="c-ok">测得</td><td class="num">0.012</td><td class="num">3.42</td></tr><tr><td class="num">2</td><td class="c-ok">测得</td><td class="num">-0.031</td><td class="num">3.45</td></tr><tr><td class="num">3</td><td class="c-ok">测得</td><td class="num">0.004</td><td class="num">3.48</td></tr><tr><td class="num">…</td><td></td><td class="num"></td><td class="num"></td></tr><tr><td class="num">10</td><td class="c-ng">缺胶</td><td class="num">未测</td><td class="num c-ng">3.91（候选，未通过）</td></tr><tr><td class="num">15</td><td class="c-ok">测得</td><td class="num">0.018</td><td class="num">3.40</td></tr></tbody></table></div>''' if open_ else ""
    return f'''<details class="wp-trial-details"{" open" if open_ else ""}>
              <summary>{ic("chevron-down",16)}逐点试测结果 · 测得 14 / 15 · 未测 1<span class="wp-trial-legend muted"><i class="ok"></i>有效胶宽<i class="ng"></i>候选胶宽未通过<span class="long">（仅供参考，不影响判定）</span></span></summary>{body}
            </details>'''

def settings(cal_open=False, scale_stale=False, editing=False):
    badge = '<span class="qs-badge warn"><span class="dot"></span>需重新验证</span>' if scale_stale else '<span class="qs-badge ok"><span class="dot"></span>已验证</span>'
    summary = ("0.0425 mm/px" if scale_stale else "0.0421 mm/px") + " · 误差 +0.032 mm"
    stale_hint = '<p class="wp-settings-hint c-warn" role="status">当量已修改，先运行试测保存，再重新验证。</p>' if scale_stale else ""
    reuse_dis = " disabled" if editing else ""
    tail = "" if editing or scale_stale else '<p class="wp-settings-hint">填写参考与实测尺寸后可记录。</p>'
    body = f'''<div class="wp-settings-body">
                <div class="wp-scale-field"><label class="qs-field"><span>像素当量</span><span class="qs-input num"><input value="{"0.0425" if scale_stale else "0.0421"}"><span class="unit">mm/px</span></span></label><a class="wp-calibration-link" href="#">飞拍标定{ic("arrow-up-right",13)}</a></div>
                {stale_hint}
                <div class="wp-form-grid"><label class="qs-field"><span>参考尺寸</span><span class="qs-input num"><input value="10.00"><span class="unit">mm</span></span></label><label class="qs-field"><span>实测尺寸</span><span class="qs-input num"><input placeholder=""><span class="unit">mm</span></span></label></div>
                <div class="wp-calibration-error"><div><span>已记录误差</span><strong>+0.032 mm</strong></div><span>目标 ±0.1 mm</span></div>
                <div class="wp-calibration-reuse"><label class="qs-check{" disabled" if editing else ""}"><input type="checkbox"{reuse_dis}>复用到其他适用拍照点</label>
                  <details><summary>适用条件{ic("chevron-down",12)}</summary><p>仅限同设备、同图且成像几何适用的其他拍照点；勾选表示已人工确认。</p></details></div>
                <button class="qs-btn filled" disabled>记录验证</button>
                {tail}
              </div>''' if cal_open else ""
    return f'''<div class="wp-stack wp-teach-settings">
            <details class="wp-settings-card"{" open" if cal_open else ""}>
              <summary><span class="wp-settings-heading"><span class="wp-settings-title">标定</span>{badge}{ic("chevron-down",16)}</span><span class="wp-settings-summary">{summary}</span></summary>
              {body}
            </details>
            <details class="wp-settings-card">
              <summary><span class="wp-settings-heading"><span class="wp-settings-title">检测参数</span><span class="qs-badge neutral"><span class="dot"></span>配方默认</span>{ic("chevron-down",16)}</span><span class="wp-settings-summary">暗胶 · 2.0–5.0 mm · 搜索 ±3.0 mm</span></summary>
            </details>
            <details class="wp-settings-card">
              <summary><span class="wp-settings-heading"><span class="wp-settings-title">判定限值</span><span class="qs-badge neutral"><span class="dot"></span>配方默认</span>{ic("chevron-down",16)}</span><span class="wp-settings-summary">位置 · 胶宽 · 缺胶</span></summary>
            </details>
          </div>'''

RAIL_EMPTY = '<div class="wp-rail-empty">选定检测图后，这里显示<br>标定 · 检测参数 · 判定限值</div>'

def dock(trial, badge_tone, badge, hint, save, nxt, tooltip=None, hint_tone=""):
    def btn(spec, icon=None, trailing=None, tip=None, align="center"):
        label, variant, disabled = spec
        cls = "qs-btn " + ("" if variant == "tonal" else variant) + (" with-leading" if icon else "")
        inner = (ic(icon,15) if icon else "") + label + (ic(trailing,15) if trailing else "")
        b = f'<button class="{cls.strip()}"{" disabled" if disabled else ""}>{inner}</button>'
        if tip:
            pos = "left:0" if align == "start" else ("right:0" if align == "end" else "left:50%;transform:translateX(-50%)")
            b = f'<span class="rel" tabindex="0">{b}<span class="qs-tooltip" style="{pos};bottom:44px">{tip}</span></span>'
        return b
    t = tooltip or (None, None)
    return f'''<div class="wp-teach-dock" role="group" aria-label="单帧示教操作">
          {btn(trial, "play", tip=t[1] if t[0]=="trial" else None, align="start")}
          <div class="wp-dock-mid"><span class="qs-badge {badge_tone}"><span class="dot"></span>{badge}</span><span class="wp-dock-hint{" warn" if hint_tone=="warn" else ""}">{hint}</span></div>
          <div class="wp-dock-actions">
            {btn(save, "save", tip=t[1] if t[0]=="save" else None)}
            {btn(nxt, None, "arrow-right", tip=t[1] if t[0]=="next" else None, align="end")}
          </div>
        </div>'''

def page(title, body_cls, groups, mid, right, dock_html, step_status="4 / 12 组", step_v=33):
    pop = ""
    if '<div class="qs-popover">' in mid:
        i = mid.index('<div class="qs-popover">'); j = mid.index('</p></div>', i) + len('</p></div>')
        pop, mid = mid[i:j], mid[:i] + mid[j:]
    return f'''<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>{title}</title>
<link rel="stylesheet" href="../teaching-design.css">
<script src="../shell.js"></script>
</head>
<body class="v2 v3{body_cls}">
<div class="app-shell">
  <main class="main">
    <header class="main-header"><h1>配方</h1><a class="site-badge" href="#">真机</a></header>
    <div class="main-body">
      <div class="wp-page">
        <div class="wp-workbench-bar">
          <label class="wp-recipe-picker">当前配方<span class="qs-input"><select><option>RCP-2024-018 · 后盖涂胶</option></select></span></label>
          <span class="qs-badge ok"><span class="dot"></span>生产 v3</span>
          <span class="qs-badge neutral"><span class="dot"></span>候选 编辑中 · 未发布</span>
        </div>
        <nav class="wp-steps" aria-label="配方配置步骤">
          <a class="wp-step-home" href="#"><span class="wp-step-no">{ic("layout-dashboard",13)}</span>总览</a>
          <a href="#"><span class="wp-step-no">1</span>整圈采集<span class="wp-step-status done">{ic("check",12)}完成</span></a>
          <a class="optional" href="#"><span class="wp-step-no">2</span>默认参数<span class="wp-step-status done">{ic("check",12)}完成</span></a>
          <a class="active" href="#"><span class="wp-step-no">3</span>单帧示教<span class="wp-step-status inProgress">{step_status}</span><span class="wp-step-progress qs-progress" style="--v:{step_v}%"><i></i></span></a>
          <a class="optional" href="#"><span class="wp-step-no">4</span>工件总览<span class="wp-step-status waiting">未开始</span></a>
          <a href="#"><span class="wp-step-no">5</span>验证与发布<span class="wp-step-status waiting">未开始</span></a>
        </nav>
        <div class="wp-teach">
          <section class="wp-group-panel" aria-label="拍照组"><h2>拍照组 <small>12</small></h2><nav class="wp-group-list" aria-label="拍照组选择">
{groups}
          </nav></section>
          <div class="wp-stack wp-mid">
            {mid}
          </div>
          {right}
          {pop}
        </div>
        {dock_html}
      </div>
    </div>
  </main>
</div>
</body>
</html>
'''

def viewport(tb, pr, cv): return f'<section class="wp-viewport" aria-label="中线示教">{tb}{pr}{cv}</section>'
PASSED = ("ok","合格","97.5%","212 ms")

pages = {}
# a 主状态
pages["a-teaching-draft"] = page("v3-a 单帧示教 · S005 刚提取中线 · 已修改需试测", "",
    groups_html("S005", {"S005":("warn","未配置")}, disabled=True),
    picker("S005", disabled=True) + viewport(toolbar("edit"), params("edit", "中线已提取，请检查后运行试测；点击图像加点（绿圈为起点），拖动点可调整，Ctrl＋滚轮缩放。"), canvas("draft")),
    settings(cal_open=True),
    dock(("运行试测","filled",False), "warn", "中线已修改 · 需试测", "有未试测的修改：先试测或清空后再切组、换图；试测会先保存中线与参数到候选", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True), tooltip=("save","试测通过后可保存")))
# b 试测通过（逐点表展开，中栏自己滚；Popover 向下弹）
pages["b-trial-passed"] = page("v3-b 单帧示教 · S005 试测通过", "",
    groups_html("S005", {"S005":("warn","试测通过")}),
    picker("S005", info_open=True) + viewport(toolbar("edit"), params("edit"), canvas("trial", band=True, trial=PASSED)) + trial_details(False),
    settings(),
    dock(("重新试测","tonal",False), "warn", "试测通过 · 未保存", "保存进候选配方，已发布 v3 不变；下一组 S006 未配置", ("保存本组示教","text",False), ("保存并进入下一组 S006","filled",False)))
# b2 逐点表展开后的滚动态
pages["b2-trial-table-open"] = page("v3-b2 单帧示教 · 逐点试测结果展开（中栏滚动）", "",
    groups_html("S005", {"S005":("warn","试测通过")}),
    picker("S005") + viewport(toolbar("edit"), params("edit"), canvas("trial", band=True, trial=PASSED)) + trial_details(True),
    settings(),
    dock(("重新试测","tonal",False), "warn", "试测通过 · 未保存", "保存进候选配方，已发布 v3 不变；下一组 S006 未配置", ("保存本组示教","text",False), ("保存并进入下一组 S006","filled",False)))
# c 1280 刷选模式
pages["c-narrow-1280"] = page("v3-c 单帧示教 · 1280×800 · 刷选模式（已修改需试测）", " narrow",
    groups_html("S005", {"S005":("warn","未配置")}, disabled=True),
    picker("S005", disabled=True) + viewport(toolbar("brush", primary="extract"), params("brush", "按住左键沿胶路从起点刷到终点，每笔重画选区；笔刷覆盖胶路和两侧少量背景。"), canvas("draft", mode="brush", footer_text="按住左键沿胶路刷过")),
    settings(),
    dock(("运行试测","filled",False), "warn", "中线已修改 · 需试测", "有未试测的修改：先试测或清空后再切组", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True)))
# c2 1280 试测通过
pages["c2-narrow-1280-passed"] = page("v3-c2 单帧示教 · 1280×800 · 试测通过", " narrow",
    groups_html("S005", {"S005":("warn","试测通过")}),
    picker("S005") + viewport(toolbar("edit"), params("edit"), canvas("trial", band=True, trial=PASSED)) + trial_details(False),
    settings(),
    dock(("重新试测","tonal",False), "warn", "试测通过 · 未保存", "保存进候选，已发布 v3 不变", ("保存本组示教","text",False), ("保存并进入下一组 S006","filled",False)))
# d 提取两次失败
pages["d-extract-failed"] = page("v3-d 单帧示教 · 自动提取两次失败 · 改用笔划生成", "",
    groups_html("S005", {"S005":("warn","未配置")}),
    picker("S005") + viewport(toolbar("brush", primary="stroke", stroke_disabled=False), params("brush", "<b>未找到连续的两侧胶边（断胶 / 胶堆）。</b>加大笔刷后仍无法连续提取：请点「笔划直接生成中线」，再手工修正。", "warn"), canvas("failed", mode="brush", points=0, footer_text="按住左键沿胶路刷过").replace("中线 <b>1</b> 点 · <b>182.4</b> mm","中线 <b>0</b> 点 · 刷选 1 笔 · 笔刷 160 px")),
    settings(),
    dock(("运行试测","tonal",True), "neutral", "待示教", "自动提取两次失败：按笔划生成中线后手工修正，再运行试测", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True), tooltip=("trial","请先提取或绘制至少两个中线点")))
# e 需重新采集：中线仍可编辑/提取，只禁试测与保存
pages["e-recapture"] = page("v3-e 单帧示教 · S005 需重新采集（可改中线，不能试测）", "",
    groups_html("S005", {"S005":("warn","需重采")}),
    picker("S005", no_link=True) +
    f'<div class="qs-alert warn" role="alert">{ic("alert-triangle",16)}<span><strong>需重新采集</strong>cam1 的触发接线或视角数在采集后被修改，当前检测图与设备不一致。中线仍可修改，重新采集后才能试测。</span><a class="qs-btn sm tonal with-leading" href="#">{ic("refresh-cw",14)}整圈重新采集</a></div>' +
    viewport(toolbar("edit"), params("edit"), canvas("draft", trial=("warn","试测已过期",None,None))),
    settings(),
    dock(("运行试测","tonal",True), "warn", "需重新采集", "图像或设备已变化：可修改中线，重新采集后才能试测", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True), tooltip=("trial","图像或设备已变化，请重新采集")))
# f 最后一组
last = {g:("ok","已示教") for g,_,_ in GROUPS}; last["S012"] = ("warn","试测通过")
pages["f-last-group"] = page("v3-f 单帧示教 · 最后一组 S012 试测通过", "",
    groups_html("S012", last),
    picker("S012") + viewport(toolbar("edit"), params("edit"), canvas("trial", band=True, trial=PASSED, group="S012")) + trial_details(False),
    settings(),
    dock(("重新试测","tonal",False), "warn", "试测通过 · 未保存", "已是最后一组；保存后 12 组全部完成，可到 <a href=\"#\">工件总览</a> 核对位置", ("保存本组示教","filled",False), ("保存并进入下一组","tonal",True), tooltip=("next","已是最后一组")),
    step_status="11 / 12 组", step_v=92)
# g 生产中 + 改了中线未试测
pages["g-production-editing"] = page("v3-g 单帧示教 · 工件检测中 · 中线已修改未试测", " tipdemo",
    groups_html("S005", {"S005":("warn","已示教")}, disabled=True, tooltip_on="S006"),
    picker("S005", disabled=True, production=True) + viewport(toolbar("edit"), params("edit", "点击图像加点（绿圈为起点），拖动点可调整，Ctrl＋滚轮缩放。"), canvas("draft", points=15, trial=("warn","试测已过期",None,None))),
    settings(cal_open=True, scale_stale=True, editing=True),
    dock(("运行试测","filled",False), "warn", "中线已修改 · 需试测", "有未试测的修改：先试测或清空后再切组、换图；现有试测不用于保存", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True)))
# h 未选检测图：中栏只有选图条 + Notice，右栏不渲染
pages["h-no-image-selected"] = page("v3-h 单帧示教 · S007 未选检测图", "",
    groups_html("S007"),
    picker("S007", selected=None) +
    f'<div class="qs-alert info-plain" role="status">{ic("info",16)}<span><strong>选择本组检测图</strong><span class="muted">点上方一张图作为 S007 的检测图开始示教，每组只需完成这一张；其余图仅采集、不检测。</span></span></div>',
    RAIL_EMPTY,
    dock(("运行试测","tonal",True), "neutral", "待选图", "先选择本组检测图", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True), tooltip=("trial","请先选择本组检测图")))
# i 多选告警
pages["i-multi-selected"] = page("v3-i 单帧示教 · S003 原先选了多张图", "",
    groups_html("S003", {"S003":("warn","需重选图")}),
    picker("S003", selected=None, multi=True) +
    f'<div class="qs-alert warn" role="alert">{ic("alert-triangle",16)}<span><strong>本组原先选择了多张图</strong>cam1 与 cam2 都被标为检测图（旧版本配方允许多选）。请重新点选一张作为 S003 的检测图，另一张自动改为仅采集。</span></div>',
    RAIL_EMPTY,
    dock(("运行试测","tonal",True), "warn", "需重选图", "本组选了多张图，请重新选择一张作为检测图", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True)))

for name, html in pages.items():
    with open(os.path.join(HERE, name + ".html"), "w", encoding="utf-8") as f: f.write(html)
print("built", len(pages))
