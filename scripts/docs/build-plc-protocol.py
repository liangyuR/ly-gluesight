import argparse
import html
import json
from pathlib import Path
import re

from markdown_it import MarkdownIt


ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SOURCE = ROOT / "docs/integration/plc-communication-protocol.md"
TARGET = SOURCE.with_suffix(".html")
MERMAID = ROOT / "node_modules/mermaid"


def load_mermaid():
    pinned = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))["devDependencies"]["mermaid"]
    try:
        installed = json.loads((MERMAID / "package.json").read_text(encoding="utf-8"))["version"]
        bundle = (MERMAID / "dist/mermaid.min.js").read_text(encoding="utf-8")
    except FileNotFoundError:
        raise SystemExit("缺少 mermaid，请先在仓库根目录运行 pnpm install")
    if installed != pinned:
        raise SystemExit(f"node_modules 中的 mermaid 为 {installed}，package.json 锁定 {pinned}，请运行 pnpm install")
    if re.search(r"</?script", bundle, re.I):
        raise SystemExit("mermaid.min.js 含 script 标签文本，不能直接内联")
    return installed, bundle


def render_fence(renderer, tokens, index, options, env):
    token = tokens[index]
    if token.info.strip() != "mermaid":
        return renderer.fence(tokens, index, options, env)
    return f'<div class="diagram-scroll" tabindex="0" role="region" aria-label="时序图，窄屏可横向滚动"><pre class="mermaid">{html.escape(token.content)}</pre></div>\n'


def render_document():
    md = MarkdownIt("commonmark", {"html": False}).enable("table")
    md.add_render_rule("fence", render_fence)
    tokens = md.parse(SOURCE.read_text(encoding="utf-8"))[3:]
    headings = []
    for index, token in enumerate(tokens):
        if token.type != "heading_open":
            continue
        label = tokens[index + 1].content
        number = re.match(r"(\d+(?:\.\d+)?)\s", label).group(1)
        ident = "section-" + number.replace(".", "-")
        token.attrSet("id", ident)
        headings.append((index, token.tag, ident, label))
    for index, token in enumerate(tokens):
        if token.type == "paragraph_open" and index + 1 < len(tokens):
            value = tokens[index + 1].content
            if value.startswith("**"):
                token.attrSet("class", "callout " + ("requirement" if value.startswith(("**发脉冲", "**有效结果")) else "warning"))
        if token.type == "link_open":
            href = token.attrGet("href") or ""
            if href == "plc-s7-phase1.md":
                token.attrSet("title", "仓库中的 S7 实现说明")
    cuts = [item for item in headings if item[1] == "h2"]
    overview = md.renderer.render(tokens[:cuts[0][0]], md.options, {})
    sections = []
    for position, (start, _, ident, _) in enumerate(cuts):
        end = cuts[position + 1][0] if position + 1 < len(cuts) else len(tokens)
        body = md.renderer.render(tokens[start:end], md.options, {})
        point_table = ident in ("section-2", "section-3")
        body = body.replace("<table>", '<div class="table-scroll" tabindex="0"><table' + (' class="point-table"' if point_table else "") + ">")
        body = body.replace("</table>", "</table></div>")
        toolbar = ""
        if ident == "section-2":
            count = len(re.findall(r"^\| DB\d+\.", SOURCE.read_text(encoding="utf-8"), re.M))
            toolbar = f'<div class="point-search"><label for="point-query">搜索两方向点表</label><div class="search-controls"><input id="point-query" type="search" placeholder="名称 / 地址 / 含义，例如 ackSeq" autocomplete="off"><button id="clear-query" type="button">清除</button><output id="point-count" aria-live="polite">{count} / {count} 个字段</output></div><p id="point-empty" hidden>没有匹配字段，请调整关键词。</p></div>'
        sections.append(f'<section class="protocol-section" aria-labelledby="{ident}">{toolbar}{body}</section>')
    nav = '<a href="#overview">协议概览</a>'
    for _, tag, ident, label in headings:
        nav += f'<a href="#{ident}"' + (' class="sub"' if tag == "h3" else "") + f'>{html.escape(label)}</a>'
    return overview, "\n".join(sections), nav


def build():
    source = SOURCE.read_text(encoding="utf-8")
    revision, reviewed, version = re.search(r"文档修订：([\d.]+)；审核日期：([\d-]+)；线上握手协议版本：(\d+)", source).groups()
    field_count = len(re.findall(r"^\| DB\d+\.", source, re.M))
    overview, sections, nav = render_document()
    mermaid_version, mermaid = load_mermaid()
    css = (HERE / "plc-protocol.css").read_text(encoding="utf-8")
    js = (HERE / "plc-protocol.js").read_text(encoding="utf-8")
    return f'''<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src 'none'; connect-src 'none'; base-uri 'none'; form-action 'none'">
<meta name="description" content="GlueSight PLC 通讯协议 {revision}：S7-200 SMART 单设备点表、三目飞拍、生产与示教采集和故障复位。">
<title>GlueSight PLC 通讯协议 {revision}</title>
<style>{css}</style>
</head>
<body>
<a class="skip" href="#overview">跳转到协议正文</a>
<div class="layout">
<aside class="rail" aria-label="协议导航">
<a class="brand" href="#overview"><span class="brand-symbol">G</span><span>GlueSight<small>PLC 通讯协议</small></span></a>
<div class="rail-meta"><span>文档 {revision}</span><span>S7 协议 {version}</span></div>
<nav aria-label="章节目录">{nav}</nav>
<div class="rail-foot"><span>现场接入 · {reviewed}</span><a href="plc-communication-protocol.md">Markdown 原文</a><a href="../../scripts/s7-handshake/phase1_plc.scl">PLC SCL 参考程序</a></div>
</aside>
<main id="content">
<header id="overview" class="hero">
<div class="hero-top"><p class="eyebrow">现场接入 / S7 ISO-on-TCP</p><button id="print-protocol" class="print-button" type="button">打印 / 保存 PDF</button></div>
<h1>PLC 通讯协议</h1>
<p class="subtitle">连续点胶飞拍 · 单件事务 · 结果确认与故障恢复</p>
<div class="facts"><div><span>共享数据区</span><strong>DB1 <small>200 SMART V 区</small></strong></div><div><span>最小区域长度</span><strong>211 <small>字节</small></strong></div><div><span>双向点表</span><strong>{field_count} <small>字段</small></strong></div><div><span>网络服务</span><strong>TCP 102</strong></div></div>
<div class="overview-text">{overview}</div>
</header>
{sections}
<footer class="document-foot"><p>协议正文与时序图均由 Markdown {revision} 生成，时序图取自正文中的 Mermaid 源，与线上握手协议 {version} 同步。现场 CPU、接线、SDK、节拍与实物检测须单独验收。</p><p>正文、样式、交互脚本和 Mermaid {mermaid_version} 渲染脚本已嵌入单个 HTML，可离线阅读。相关源码链接使用仓库相对路径。</p></footer>
</main>
</div>
<script>{mermaid}</script>
<script>{js}</script>
</body>
</html>
'''


def main():
    parser = argparse.ArgumentParser(description="从已审核 Markdown（含 Mermaid 时序图）生成自包含 PLC 协议 HTML。修改时序图请直接编辑 Markdown 中的 mermaid 代码块。依赖安装：先在仓库根目录运行 pnpm install 安装锁定版本的 mermaid，再运行 python -m pip install -r scripts/docs/requirements.txt")
    parser.add_argument("--check", action="store_true", help="核对 HTML 是否与当前 Markdown、样式、交互脚本及内嵌 mermaid 同步，不写文件")
    args = parser.parse_args()
    content = build()
    if args.check:
        if not TARGET.exists() or TARGET.read_text(encoding="utf-8") != content:
            parser.exit(1, "HTML 尚未同步，请运行 python scripts/docs/build-plc-protocol.py\n")
        print("PASS: HTML 与 Markdown、Mermaid 时序图、样式、交互脚本及内嵌 mermaid 一致")
    else:
        TARGET.write_text(content, encoding="utf-8", newline="\n")
        print(f"已生成：{TARGET}")


if __name__ == "__main__":
    main()
