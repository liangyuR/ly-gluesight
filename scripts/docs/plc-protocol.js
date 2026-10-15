mermaid.initialize({
  startOnLoad: true,
  securityLevel: 'strict',
  theme: 'base',
  fontFamily: 'system-ui,-apple-system,"Segoe UI","Microsoft YaHei",sans-serif',
  themeVariables: {
    fontFamily: 'system-ui,-apple-system,"Segoe UI","Microsoft YaHei",sans-serif',
    fontSize: '15px',
    primaryColor: '#edf4fb',
    primaryTextColor: '#21364a',
    primaryBorderColor: '#245b96',
    lineColor: '#245b96',
    textColor: '#21364a',
    actorBkg: '#edf4fb',
    actorBorder: '#245b96',
    actorTextColor: '#21364a',
    actorLineColor: '#b6c8d6',
    signalColor: '#245b96',
    signalTextColor: '#21364a',
    noteBkgColor: '#f1f8fc',
    noteBorderColor: '#cfe0ed',
    noteTextColor: '#21364a',
    labelBoxBkgColor: '#edf8f5',
    labelBoxBorderColor: '#13806d',
    labelTextColor: '#13806d',
    loopTextColor: '#13806d'
  },
  sequence: { useMaxWidth: true, mirrorActors: true, messageAlign: 'center', noteAlign: 'left' }
});
const query = document.getElementById('point-query');
const rows = [...document.querySelectorAll('.point-table tbody tr')];
function filterPoints() {
  const term = query.value.trim().toLocaleLowerCase();
  rows.forEach(row => { row.hidden = !row.textContent.toLocaleLowerCase().includes(term); });
  const count = rows.filter(row => !row.hidden).length;
  document.getElementById('point-count').textContent = `${count} / ${rows.length} 个字段`;
  document.getElementById('point-empty').hidden = count !== 0;
}
query.addEventListener('input', filterPoints);
document.getElementById('clear-query').addEventListener('click', () => {
  query.value = '';
  filterPoints();
  query.focus();
});
document.getElementById('print-protocol').addEventListener('click', () => window.print());
const navLinks = [...document.querySelectorAll('.rail nav a')];
const anchors = navLinks.map(link => document.getElementById(link.hash.slice(1)));
let scheduled = false;
function updateNavigation() {
  let current = 0;
  anchors.forEach((anchor, index) => { if (anchor.getBoundingClientRect().top < 150) current = index; });
  navLinks.forEach((link, index) => {
    if (index === current) link.setAttribute('aria-current', 'true');
    else link.removeAttribute('aria-current');
  });
  scheduled = false;
}
window.addEventListener('scroll', () => {
  if (!scheduled) { scheduled = true; requestAnimationFrame(updateNavigation); }
}, { passive: true });
updateNavigation();
