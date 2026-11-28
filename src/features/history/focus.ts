export function focusInHistory(element: HTMLElement) {
  element.focus({ preventScroll: true });
  const content = element.closest<HTMLElement>(".main-body");
  if (content) content.scrollTop += element.getBoundingClientRect().top - content.getBoundingClientRect().top - 8;
}
