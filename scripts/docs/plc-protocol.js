const tabs = [...document.querySelectorAll('.diagram-tabs [role="tab"]')];
const panels = [...document.querySelectorAll('.diagram-panel')];
function selectDiagram(index, focus = false) {
  tabs.forEach((tab, i) => {
    tab.setAttribute('aria-selected', String(i === index));
    tab.tabIndex = i === index ? 0 : -1;
    panels[i].hidden = i !== index;
  });
  if (focus) tabs[index].focus();
}
tabs.forEach((tab, index) => {
  tab.addEventListener('click', () => selectDiagram(index));
  tab.addEventListener('keydown', event => {
    let next;
    if (event.key === 'ArrowRight') next = (index + 1) % tabs.length;
    if (event.key === 'ArrowLeft') next = (index + tabs.length - 1) % tabs.length;
    if (event.key === 'Home') next = 0;
    if (event.key === 'End') next = tabs.length - 1;
    if (next !== undefined) {
      event.preventDefault();
      selectDiagram(next, true);
    }
  });
});
selectDiagram(0);
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
