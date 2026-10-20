# -*- coding: utf-8 -*-
"""生成 v2 各状态的静态 HTML（python build.py）。页面结构见 README「v2」。"""
import os
HERE = os.path.dirname(os.path.abspath(__file__))

GROUPS = [("S001","ok","已示教"),("S002","ok","已示教"),("S003","ok","已示教"),("S004","ok","已示教"),("S005","warn","待试测"),("S006","warn","未配置"),
          ("S007","off","待选图"),("S008","off","待选图"),("S009","off","待选图"),("S010","off","待选图"),("S011","off","待选图"),("S012","off","待选图")]

def ic(name, size=16): return f'<i data-icon="{name}" data-size="{size}"></i>'

def groups_html(selected, status_override=None, disabled=False, tooltip_on=None):
    out = []
    for gid, tone, status in GROUPS:
        if status_override and gid in status_override: tone, status = status_override[gid]
        sel = " selected" if gid == selected else ""
        dis = " disabled" if disabled and gid != selected else ""
        tip = f'<span class="qs-tooltip below start" style="left:0;top:38px">有未试测的修改，先试测或清空后再切组</span>' if tooltip_on == gid else ""
        wrap = ' class="rel block"' if tip else ""
        out.append(f'<span{wrap}><button class="wp-group-item{sel}"{dis}{" aria-pressed=\"true\"" if sel else ""}><i class="dot {tone}"></i><strong>{gid}</strong><span>{status}</span></button>{tip}</span>' if tip else
                   f'<button class="wp-group-item{sel}"{dis}{" aria-pressed=\"true\"" if sel else ""}><i class="dot {tone}"></i><strong>{gid}</strong><span>{status}</span></button>')
    return "\n".join(out)

def picker(group, disabled=False, recapture=False, production=False, info_open=False, mismatch=True):
    dis = " disabled" if disabled else ""
    thumbs = "".join(
        f'<button class="wp-view-option{" selected" if i == 0 else ""}"{dis}{" aria-pressed=\"true\"" if i==0 else ""}><span class="wp-view-thumbnail" data-bead="{i+1}"></span><span class="wp-view-caption"><strong>cam{i+1} · 图 1</strong><span class="small">{"当前检测图" if i==0 else "选用此图"}</span></span></button>'
        for i in range(3))
    side = ""
    pop = ""
    if info_open:
        pop = ('<div class="qs-popover" style="right:12px;top:46px"><h3>当前检测图 · S005 · cam1 · 图 1</h3>'
               '<div class="wp-kv"><span>图像</span><strong class="mono">img_0f3a2c</strong></div><div class="wp-kv"><span>取样来源</span><strong>整圈采集 · 2026/10/18 14:02</strong></div>'
               '<div class="wp-kv"><span>曝光 / 增益</span><strong>1200 μs / 6 dB</strong></div><div class="wp-kv"><span>原图更新</span><strong>仅通过整圈重新采集</strong></div>'
               '<p class="c-warn">采集时曝光 1200 µs / 增益 6 dB，与配方当前值（1500 µs / 6 dB）不同；样本仍可用，需要时整圈重采。</p>' if mismatch else ''
               '<p>中线保存进候选配方，已发布版本不变。用于生产前仍需完成标定、代表性样本验证与发布。</p></div>')
    side += f'<button class="qs-icon-btn sm" aria-label="图像采集信息" aria-expanded="{"true" if info_open else "false"}">{ic("info",15)}</button>'
    if production:
        side += f'<span class="wp-lock-note">{ic("lock",13)}工件检测中，结束后可重采</span>'
    elif recapture:
        side += f'<a class="qs-btn sm tonal with-leading" href="#">{ic("refresh-cw",14)}重新采集</a>'
    else:
        side += '<a class="wp-text-link" href="#">重新采集</a>'
    return f'''<section class="wp-picker" aria-label="本组检测图选择">
              <h2>{group}<small>检测图</small></h2>
              <div class="wp-group-images" role="group" aria-label="本组图像">{thumbs}</div>
              <div class="wp-picker-side">{side}</div>{pop}
            </section>'''

def toolbar(mode="edit", primary=None, extract_disabled=False, stroke_disabled=True, stroke_tip=None):
    seg = "".join(f'<button{" aria-pressed=\"true\"" if mode == m else ""}>{label}</button>' for m, label in [("edit","编辑点"),("brush","沿胶路刷选"),("box","矩形框选")])
    def variant(k): return "filled" if primary == k else ""
    stroke_btn = f'<button class="qs-btn sm {variant("stroke")}"{" disabled" if stroke_disabled else ""}>笔划直接生成中线</button>'
    if stroke_tip: stroke_btn = f'<span class="rel">{stroke_btn}<span class="qs-tooltip" style="left:50%;top:38px;transform:translateX(-50%)" >{stroke_tip}</span></span>'
    return f'''<div class="wp-image-tools">
                <div class="wp-canvas-tools" role="group" aria-label="示教工具">
                  <div class="qs-seg" aria-label="画布模式">{seg}</div>
                  <button class="qs-btn sm with-leading {variant("extract")}"{" disabled" if extract_disabled else ""}>{ic("wand-sparkles",14)}自动提取中线</button>
                  {stroke_btn}
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
        if hint_tone == "warn": h = f'<p class="wp-canvas-hint warn" role="status">{ic("alert-triangle",14)}<span>{hint}</span></p>'
        else: h = f'<p class="wp-canvas-hint" role="status">{hint}</p>'
    return f'''<div class="wp-canvas-params">
                {brush}
                <span class="wp-inline-num">中线点间距<span class="qs-input sm num"><input value="20"><span class="unit">px</span></span></span>
                <button class="qs-btn sm">应用到当前中线</button>
                {h}
              </div>'''

def canvas(overlay="draft", points=14, dim=False, cover=None, footer_text="点击图像加中线点（绿圈为起点），拖动点可调整", band=False, group="S005"):
    cov = f'<div class="wp-canvas-cover"><strong>{ic("alert-triangle",14)}{cover[0]}</strong><span>{cover[1]}</span></div>' if cover else ""
    ov = f' data-overlay="{overlay}" data-points="{points}"' if overlay else ""
    return f'''<div class="wp-gray-image{" dim" if dim else ""}" data-bead="1"{ov}>{cov}</div>
              <div class="wp-image-footer"><span><strong>{group} · cam1 · 图 1</strong> · 2448 × 2048 px · {footer_text} · Ctrl＋滚轮缩放 · 中键拖动</span>
                <div class="wp-zoom"><button class="qs-icon-btn sm" aria-label="缩小">{ic("minus",15)}</button><span class="mono">100%</span><button class="qs-icon-btn sm" aria-label="放大">{ic("plus",15)}</button><button class="qs-icon-btn sm" aria-label="适应窗口">{ic("rotate-ccw",15)}</button>
                <button class="qs-chip selected">中线叠加</button><button class="qs-chip{" selected" if band else ""}">搜索带</button><button class="qs-chip selected">编辑点</button></div>
              </div>'''

def trial_bar(kind="none", stale=False):
    if kind == "none":
        return '<div class="wp-trial-bar"><span class="qs-badge neutral"><span class="dot"></span>未试测</span><span class="muted">中线 15 点 · 182.4 mm</span></div>'
    if kind == "passed":
        return ('<div class="wp-trial-bar"><span class="wp-trial-result" aria-label="本次算法结果"><span class="qs-badge ok"><span class="dot"></span>合格</span><span>量成比例 <b>97.5%</b></span><span>耗时 <b>212 ms</b></span></span>'
                '<span class="muted">中线 15 点 · 182.4 mm · 15 站</span>'
                '<span class="wp-trial-legend muted"><i class="ok"></i>有效胶宽<i class="ng"></i>候选胶宽未通过（仅供参考，不影响判定）</span></div>')
    if kind == "stale":
        return '<div class="wp-trial-bar"><span class="qs-badge warn"><span class="dot"></span>试测已过期</span><span class="muted">中线 16 点 · 184.1 mm</span><span class="muted">上次：合格 · 97.5% · 212 ms（中线已改，不再用于保存）</span></div>'
    return ""

TRIAL_TABLE = '''<details class="wp-trial-details" open>
                <summary>逐点试测结果 · 测得 14 / 15 · 未测 1</summary>
                <p class="hint">绿色横线为有效胶宽，红色横线为未通过的候选胶宽，仅供参考，不改变 NG 判定；无候选胶边时保留红点。</p>
                <table class="qs-table"><thead><tr><th class="num">测量站</th><th>状态</th><th class="num">偏移 mm</th><th class="num">胶宽 mm</th></tr></thead>
                <tbody><tr><td class="num">1</td><td class="c-ok">测得</td><td class="num">0.012</td><td class="num">3.42</td></tr><tr><td class="num">2</td><td class="c-ok">测得</td><td class="num">-0.031</td><td class="num">3.45</td></tr><tr><td class="num">3</td><td class="c-ok">测得</td><td class="num">0.004</td><td class="num">3.48</td></tr><tr><td class="num">…</td><td></td><td class="num"></td><td class="num"></td></tr><tr><td class="num">10</td><td class="c-ng">缺胶</td><td class="num">未测</td><td class="num c-ng">3.91（候选，未通过）</td></tr><tr><td class="num">15</td><td class="c-ok">测得</td><td class="num">0.018</td><td class="num">3.40</td></tr></tbody></table>
              </details>'''

def settings(cal_open=True, scale_stale=False):
    stale_hint = '<p class="wp-settings-hint c-warn" role="status">当量已修改，先运行试测保存，再重新验证。</p>' if scale_stale else ""
    badge = '<span class="qs-badge warn"><span class="dot"></span>需重新验证</span>' if scale_stale else '<span class="qs-badge ok"><span class="dot"></span>已验证</span>'
    summary = "0.0425 mm/px · 误差 +0.032 mm" if scale_stale else "0.0421 mm/px · 误差 +0.032 mm"
    body = f'''<div class="wp-settings-body">
                <div class="wp-scale-field"><label class="qs-field"><span>像素当量</span><span class="qs-input num"><input value="{"0.0425" if scale_stale else "0.0421"}"><span class="unit">mm/px</span></span></label><a class="wp-calibration-link" href="#">飞拍标定{ic("arrow-up-right",13)}</a></div>
                {stale_hint}
                <div class="wp-form-grid"><label class="qs-field"><span>参考尺寸</span><span class="qs-input num"><input value="10.00"><span class="unit">mm</span></span></label><label class="qs-field"><span>实测尺寸</span><span class="qs-input num"><input placeholder=""><span class="unit">mm</span></span></label></div>
                <div class="wp-calibration-error"><div><span>已记录误差</span><strong>+0.032 mm</strong></div><span>目标 ±0.1 mm</span></div>
                <div class="wp-calibration-reuse"><label class="qs-check"><input type="checkbox">复用到其他适用拍照点</label>
                  <details><summary>适用条件{ic("chevron-down",12)}</summary><p>仅限同设备、同图且成像几何适用的其他拍照点；勾选表示已人工确认。</p></details></div>
                <button class="qs-btn filled" disabled>记录验证</button>
                <p class="wp-settings-hint">填写参考与实测尺寸后可记录。</p>
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

def dock(trial, badge_tone, badge, hint, save, nxt, tooltip=None, hint_tone=""):
    """trial/save/nxt: (label, variant, disabled)。variant: filled/tonal/text。"""
    def btn(spec, icon=None, trailing=None, tip=None):
        label, variant, disabled = spec
        cls = "qs-btn " + ("" if variant == "tonal" else variant) + (" with-leading" if icon else "")
        inner = (ic(icon,15) if icon else "") + label + (ic(trailing,15) if trailing else "")
        b = f'<button class="{cls.strip()}"{" disabled" if disabled else ""}>{inner}</button>'
        if tip: b = f'<span class="rel">{b}<span class="qs-tooltip" style="{"left:0" if icon == "play" else "left:50%;transform:translateX(-50%)"};bottom:44px">{tip}</span></span>'
        return b
    return f'''<div class="wp-teach-dock" role="group" aria-label="单帧示教操作">
          {btn(trial, "play", tip=tooltip[1] if tooltip and tooltip[0]=="trial" else None)}
          <div class="wp-dock-mid"><span class="qs-badge {badge_tone}"><span class="dot"></span>{badge}</span><span class="wp-dock-hint{" warn" if hint_tone=="warn" else ""}">{hint}</span></div>
          <div class="wp-dock-actions">
            {btn(save, "save", tip=tooltip[1] if tooltip and tooltip[0]=="save" else None)}
            {btn(nxt, None, "arrow-right", tip=tooltip[1] if tooltip and tooltip[0]=="next" else None)}
          </div>
        </div>'''

def page(title, body_cls, scroll, groups, mid, right, dock_html, step_status="4 / 12 组", step_v=33):
    return f'''<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>{title}</title>
<link rel="stylesheet" href="../teaching-design.css">
<script src="../shell.js"></script>
</head>
<body class="v2{body_cls}"{f' data-scroll="{scroll}"' if scroll else ""}>
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
          <a class="active" href="#"><span class="wp-step-no">3</span>单帧示教<span class="wp-step-status inProgress">{step_status}</span><span class="wp-step-progress" style="--v:{step_v}%"><i></i></span></a>
          <a class="optional" href="#"><span class="wp-step-no">4</span>工件总览<span class="wp-step-status waiting">未开始</span></a>
          <a href="#"><span class="wp-step-no">5</span>验证与发布<span class="wp-step-status waiting">未开始</span></a>
        </nav>
        <div class="wp-teach">
          <section class="wp-group-panel" aria-label="拍照组"><h2>拍照组 <small>12</small></h2><nav class="wp-group-list" aria-label="拍照组选择">
{groups}
          </nav></section>
          <div class="wp-stack">
            {mid}
          </div>
          {right}
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

pages = {}

# a 主状态：中线草稿、待试测（编辑点模式；悬停“保存本组示教”显示禁用原因 Tooltip）
pages["a-teaching-draft"] = page("v2-a 单帧示教 · S005 中线草稿 · 待试测", "", None,
    groups_html("S005"),
    picker("S005") + viewport(toolbar("edit", primary=None), params("edit", "中线已提取，请检查后运行试测。"), canvas("draft")) + trial_bar("none"),
    settings(cal_open=True),
    dock(("运行试测","filled",False), "info", "待试测", "试测会先把中线与参数保存到候选，再对本图跑一次检测", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True), tooltip=("save","试测通过后可保存")))

# b 试测通过
pages["b-trial-passed"] = page("v2-b 单帧示教 · S005 试测通过 · 可保存并进入下一组", "", None,
    groups_html("S005", {"S005":("warn","试测通过")}),
    picker("S005", info_open=True) + viewport(toolbar("edit"), params("edit"), canvas("trial", band=True)) + trial_bar("passed") + TRIAL_TABLE,
    settings(cal_open=False),
    dock(("重新试测","tonal",False), "warn", "试测通过 · 未保存", "保存进候选配方，已发布 v3 不变；下一组 S006 未配置", ("保存本组示教","text",False), ("保存并进入下一组 S006","filled",False)))

# c 1280 宽（刷选模式，看参数行与工具栏）
pages["c-narrow-1280"] = page("v2-c 单帧示教 · 1280×800 · 刷选模式", " narrow", None,
    groups_html("S005"),
    picker("S005") + viewport(toolbar("brush", primary="extract"), params("brush", "按住左键沿胶路从起点刷到终点，每笔重画选区；笔刷覆盖胶路和两侧少量背景，尽量避开阴影。"), canvas("draft", footer_text="按住左键沿胶路刷过；每一笔重画选区")) + trial_bar("none"),
    settings(cal_open=False),
    dock(("运行试测","filled",False), "info", "待试测", "试测会先保存中线与参数到候选", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True)))

# d 提取失败两次 → 主按钮切到“笔划直接生成中线”
pages["d-extract-failed"] = page("v2-d 单帧示教 · 自动提取两次失败 · 改用笔划生成", "", None,
    groups_html("S005", {"S005":("warn","未配置")}),
    picker("S005") + viewport(toolbar("brush", primary="stroke", stroke_disabled=False), params("brush", "<b>未找到连续的两侧胶边（断胶 / 胶堆）。</b>加大笔刷后仍无法连续提取：请点「笔划直接生成中线」，再手工修正。", "warn"), canvas("failed", footer_text="按住左键沿胶路刷过；每一笔重画选区")) +
    '<div class="wp-trial-bar"><span class="qs-badge neutral"><span class="dot"></span>未试测</span><span class="muted">中线 0 点 · 刷选 1 笔 · 笔刷 160 px</span></div>',
    settings(cal_open=False),
    dock(("运行试测","tonal",True), "neutral", "待示教", "自动提取两次失败：按笔划生成中线后手工修正，再运行试测", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True), tooltip=("trial","请先提取或绘制至少两个中线点")))

# e 需重新采集（图像或设备已变化）
pages["e-recapture"] = page("v2-e 单帧示教 · S005 需重新采集", "", None,
    groups_html("S005", {"S005":("warn","需重采")}),
    picker("S005", recapture=True) +
    f'<div class="qs-alert warn" role="alert">{ic("alert-triangle",16)}<span><strong>需重新采集</strong>cam1 的触发接线或视角数在采集后被修改，当前检测图与设备不一致；已保存的中线保留，重采后需重新试测。</span><a class="qs-btn sm tonal" href="#">整圈重新采集</a></div>' +
    viewport(toolbar("edit", extract_disabled=True), params("edit"), canvas("draft", dim=True, cover=("图像或设备已变化","中线可查看，不能编辑或试测；请整圈重新采集后继续"), footer_text="只读")) +
    '<div class="wp-trial-bar"><span class="qs-badge warn"><span class="dot"></span>试测已过期</span><span class="muted">中线 15 点 · 182.4 mm · 上次合格 2026/10/18</span></div>',
    settings(cal_open=False),
    dock(("运行试测","tonal",True), "warn", "需重新采集", "图像或设备已变化，请重新采集后再试测", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True), hint_tone="warn"))

# f 最后一组 S012 试测通过（无下一组）
last_groups = {g:("ok","已示教") for g,_,_ in GROUPS}; last_groups["S012"] = ("warn","试测通过")
pages["f-last-group"] = page("v2-f 单帧示教 · 最后一组 S012 试测通过", "", None,
    groups_html("S012", last_groups),
    picker("S012") + viewport(toolbar("edit"), params("edit"), canvas("trial", band=True, group="S012")) + trial_bar("passed"),
    settings(cal_open=False),
    dock(("重新试测","tonal",False), "warn", "试测通过 · 未保存", "这是最后一组；保存后 12 组全部完成，可前往工件总览核对位置", ("保存本组示教","filled",False), ("保存并前往工件总览","tonal",False)),
    step_status="11 / 12 组", step_v=92)

# g 生产中 + 改了中线未试测（切组与选图被禁用，就近说明）
pages["g-production-editing"] = page("v2-g 单帧示教 · 工件检测中 · 中线已修改未试测（切组禁用）", "", None,
    groups_html("S005", {"S005":("warn","已示教")}, disabled=True, tooltip_on="S006"),
    picker("S005", disabled=True, production=True) + viewport(toolbar("edit"), params("edit"), canvas("draft", points=15)) + trial_bar("stale"),
    settings(cal_open=True, scale_stale=True),
    dock(("运行试测","filled",False), "warn", "中线已修改 · 需试测", "有未试测的修改：先试测或清空后再切组、换图；现有试测不用于保存", ("保存本组示教","tonal",True), ("保存并进入下一组","tonal",True)))

for name, html in pages.items():
    with open(os.path.join(HERE, name + ".html"), "w", encoding="utf-8") as f: f.write(html)
print("built", len(pages))
