import {readFile,writeFile,access} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const dir=path.join(root,'docs/user-guide');
const guide=JSON.parse(await readFile(path.join(dir,'guide.json'),'utf8'));
const escape=text=>String(text).replace(/[&<>"']/g,char=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
const total=guide.chapters.length+2;
const footer=(page)=>`<footer><span>GlueSight 图文使用教程 · ${escape(guide.edition)}</span><span>${page} / ${total}</span></footer>`;
const links=guide.chapters.map((chapter,i)=>`<a href="#${escape(chapter.id)}"><b>${String(i+1).padStart(2,'0')}</b>${escape(chapter.title)}</a>`).join('');
const table=table=>table?`<table><thead><tr>${table.headers.map(v=>`<th>${escape(v)}</th>`).join('')}</tr></thead><tbody>${table.rows.map(row=>`<tr>${row.map(v=>`<td>${escape(v)}</td>`).join('')}</tr>`).join('')}</tbody></table>`:'';
const sections=[];
for(const [i,chapter] of guide.chapters.entries()){
  for(const img of chapter.images??[])await access(path.join(dir,'images',img.src));
  const figures=(chapter.images??[]).map((img,j)=>`<figure${img.slim?' class="slim"':''}><img src="images/${escape(img.src)}" alt="${escape(img.caption)}" loading="eager"><figcaption>图 ${i+1}${chapter.images.length>1?'-'+(j+1):''}　${escape(img.caption)}</figcaption></figure>`).join('');
  sections.push(`<section class="sheet lesson${figures?'':' text-only'}" id="${escape(chapter.id)}"><p class="kicker">${escape(chapter.group)} / ${String(i+1).padStart(2,'0')}</p><h2>${i+1}　${escape(chapter.title)}</h2><p class="intro">${escape(chapter.intro)}</p>${table(chapter.table)}<ol class="steps">${chapter.steps.map((step,n)=>`<li><span class="step-number">${n+1}</span><span>${escape(step)}</span></li>`).join('')}</ol>${figures?`<div class="media">${figures}</div>`:''}<div class="note">${escape(chapter.note)}</div>${footer(i+3)}</section>`);
}
const html=`<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${escape(guide.title)}</title><meta name="description" content="${escape(guide.intro)}"><link rel="stylesheet" href="guide.css"></head><body><nav id="toc" aria-label="教程目录"><strong>GlueSight</strong><small>图文使用教程</small><div class="actions"><button id="print-guide">打印 / 保存 PDF</button></div><a href="#cover">封面</a><a href="#contents">阅读路线</a>${links}</nav><main class="document"><section class="sheet cover" id="cover"><p class="kicker">GLUESIGHT / 用户操作手册</p><h1>${escape(guide.title)}</h1><p class="subtitle">${escape(guide.subtitle)}</p><p class="intro">${escape(guide.intro)}</p><div class="pathway"><div><b>01 建站</b><span>相机 · PLC · 图像测量</span></div><div><b>02 配方</b><span>胶路规划 · 逐帧示教</span></div><div><b>03 发布</b><span>代表性样本 · 验证</span></div><div><b>04 复盘</b><span>检测结果 · 原图复测</span></div></div><div class="cover-note">${escape(guide.scope)}</div><p class="edition">${escape(guide.edition)}　|　示例 ROBOT-DEMO · 四点飞拍 · 产品代码 101</p><p class="screen-hint">点击截图可放大。左侧目录可跳转；打印时自动生成 A4 横向手册。</p>${footer(1)}</section><section class="sheet contents" id="contents"><p class="kicker">阅读路线</p><h2>按你现在要做的事开始</h2><p class="intro">已有示例先读第 1 节。首次建站按第 2 至 13 节操作。日常运行与复盘看第 14 至 18 节。随动和环境维护看最后三节。</p><ol class="contents-list">${guide.chapters.map((c,i)=>`<li><span class="num">${String(i+1).padStart(2,'0')}</span><a href="#${escape(c.id)}">${escape(c.title)}</a><span class="page">${i+3}</span></li>`).join('')}</ol><div class="note">操作顺序：保存候选 → 当前图像试测 → 保存示教 → 验证代表性样本 → 发布生产版本。修改参数后，按页面提示重新完成受影响的步骤。</div>${footer(2)}</section>${sections.join('\n')}</main><dialog id="image-viewer"><div class="lightbox-head"><span>图片</span><button>关闭</button></div><img alt=""></dialog><script src="guide.js"></script></body></html>`;
await writeFile(path.join(dir,'index.html'),html,'utf8');
console.log(`Built ${total} tutorial pages: ${path.join(dir,'index.html')}`);
