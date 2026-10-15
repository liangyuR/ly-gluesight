import argparse
import html
from pathlib import Path
import re

from markdown_it import MarkdownIt


ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SOURCE = ROOT / "docs/integration/plc-communication-protocol.md"
TARGET = SOURCE.with_suffix(".html")
DIAGRAMS = [
    ("normal", "正常生产一件", "PLC 硬触发、PC 异步检测，再以匹配序号完成结果确认与释放。", "plc-normal-sequence.svg"),
    ("early-stop", "提前终止与故障", "区分可确认的 ERR 结果与没有有效 done 的故障，保留未结事务。", "plc-early-stop-sequence.svg"),
    ("reset", "启动与显式复位", "实物处置完成后，建立中性输入并验证 ready 的低到高变化。", "plc-reset-sequence.svg"),
]


def render_document():
    md = MarkdownIt("commonmark", {"html": False}).enable("table")
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
            count = len(re.findall(r"^\| DB100\.", SOURCE.read_text(encoding="utf-8"), re.M))
            toolbar = f'<div class="point-search"><label for="point-query">搜索两方向点表</label><div class="search-controls"><input id="point-query" type="search" placeholder="名称 / 地址 / 含义，例如 ackSeq" autocomplete="off"><button id="clear-query" type="button">清除</button><output id="point-count" aria-live="polite">{count} / {count} 个字段</output></div><p id="point-empty" hidden>没有匹配字段，请调整关键词。</p></div>'
        sections.append(f'<section class="protocol-section" aria-labelledby="{ident}">{toolbar}{body}</section>')
    nav = '<a href="#overview">协议概览</a><a href="#sequence">通讯时序图</a>'
    for _, tag, ident, label in headings:
        nav += f'<a href="#{ident}"' + (' class="sub"' if tag == "h3" else "") + f'>{html.escape(label)}</a>'
    return overview, "\n".join(sections), nav


def render_diagrams():
    tabs, panels = [], []
    for index, (ident, label, description, name) in enumerate(DIAGRAMS):
        svg = (SOURCE.parent / "diagrams" / name).read_text(encoding="utf-8")
        svg = re.sub(r"<\?xml[^>]*\?>\s*", "", svg)
        if not svg.lstrip().startswith("<svg"):
            raise ValueError(f"不是 SVG：{name}")
        identifiers = re.findall(r'\bid="([^"]+)"', svg)
        for original in identifiers:
            svg = svg.replace(f'id="{original}"', f'id="{ident}-{original}"')
            svg = svg.replace(f'url(#{original})', f'url(#{ident}-{original})')
        svg = re.sub(r'aria-labelledby="([^"]+)"', lambda match: 'aria-labelledby="' + " ".join(f"{ident}-{value}" for value in match[1].split()) + '"', svg)
        svg = svg.replace("<svg ", f'<svg id="diagram-{ident}" ', 1)
        svg = re.sub(r"(<style>)(.*?)(</style>)", lambda match: match[1] + re.sub(r"([^{}]+)\{", lambda rule: ",".join(f"#diagram-{ident} {selector.strip()}" for selector in rule[1].split(",")) + "{", match[2]) + match[3], svg, flags=re.S)
        tabs.append(f'<button id="tab-{ident}" role="tab" aria-controls="panel-{ident}" aria-selected="{str(index == 0).lower()}" type="button">{html.escape(label)}</button>')
        panels.append(f'<figure class="diagram-panel" id="panel-{ident}" role="tabpanel" aria-labelledby="tab-{ident}"><figcaption><div><h3>{html.escape(label)}</h3><p>{html.escape(description)}</p></div><a class="download" href="diagrams/{name}" download>下载 SVG</a></figcaption><div class="diagram-scroll" tabindex="0" aria-label="{html.escape(label)}，窄屏可横向滚动">{svg}</div></figure>')
    return '<div class="diagram-tabs" role="tablist" aria-label="时序图场景">' + "".join(tabs) + "</div>" + "\n".join(panels)


def build():
    source = SOURCE.read_text(encoding="utf-8")
    revision, reviewed, version = re.search(r"文档修订：([\d.]+)；审核日期：([\d-]+)；线上握手协议版本：(\d+)", source).groups()
    field_count = len(re.findall(r"^\| DB100\.", source, re.M))
    overview, sections, nav = render_document()
    diagrams = render_diagrams()
    css = (HERE / "plc-protocol.css").read_text(encoding="utf-8")
    js = (HERE / "plc-protocol.js").read_text(encoding="utf-8")
    return f'''<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src 'none'; connect-src 'none'; base-uri 'none'; form-action 'none'">
<meta name="description" content="GlueSight PLC 通讯协议 {revision}：单设备 S7 点表、三目飞拍、生产与示教采集和故障复位。">
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
<div class="facts"><div><span>共享数据区</span><strong>DB100 <small>默认</small></strong></div><div><span>最小 DB 长度</span><strong>114 <small>字节</small></strong></div><div><span>双向点表</span><strong>{field_count} <small>字段</small></strong></div><div><span>网络服务</span><strong>TCP 102</strong></div></div>
<div class="overview-text">{overview}</div>
</header>
<section id="sequence" class="sequence-section" aria-labelledby="sequence-heading">
<p class="eyebrow">握手与状态交付</p><h2 id="sequence-heading">通讯时序图</h2>
<p class="callout warning"><strong>时序图待更新：</strong>按本次检查流程，以下三张图保持文档 1.1 / 协议 1 的原样，待单设备协议检查完成后统一更新。图中的旧字段名称、设备通道及版本号不适用于当前协议，请以正文和点表为准。</p>
<p class="section-intro">沿生命线向下阅读，箭头表示写入、观察或物理触发。图中纵向间距不代表实际耗时；单件握手和故障恢复按正文条件执行。</p>
{diagrams}
<p class="diagram-note">三目设备的一次触发只计一次；PC 轮询不生成位置同步触发。示教采集的 <code>1/0</code> 只表示采集完成，不能作为良品信号。参考 FB 尚缺的保护以正文接入限制为准。</p>
</section>
{sections}
<footer class="document-foot"><p>协议正文与 Markdown {revision} 同步；时序图保持旧版，待协议检查完成后统一更新。现场 CPU、接线、SDK、节拍与实物检测须单独验收。</p><p>正文和图形已嵌入单个 HTML，可离线阅读。相关源码与 SVG 下载链接使用仓库相对路径。</p></footer>
</main>
</div>
<script>{js}</script>
</body>
</html>
'''


def main():
    parser = argparse.ArgumentParser(description="从已审核 Markdown 和 SVG 生成自包含 PLC 协议 HTML。依赖安装：python -m pip install -r scripts/docs/requirements.txt")
    parser.add_argument("--check", action="store_true", help="核对 HTML 是否与当前正文、图形及样式同步，不写文件")
    args = parser.parse_args()
    content = build()
    if args.check:
        if not TARGET.exists() or TARGET.read_text(encoding="utf-8") != content:
            parser.exit(1, "HTML 尚未同步，请运行 python scripts/docs/build-plc-protocol.py\n")
        print("PASS: HTML 与 Markdown、SVG、样式及交互脚本一致")
    else:
        TARGET.write_text(content, encoding="utf-8", newline="\n")
        print(f"已生成：{TARGET}")


if __name__ == "__main__":
    main()
