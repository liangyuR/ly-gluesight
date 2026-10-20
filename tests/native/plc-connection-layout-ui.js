async (page) => {
  await page.getByRole("heading", { name: "心跳与日志", exact: true }).waitFor();
  const layout = await page.locator(".plc-config-card").evaluateAll(cards => cards.map(card => {
    const bounds = card.getBoundingClientRect();
    const fields = card.querySelector(".plc-config-fields").getBoundingClientRect();
    return { title: card.querySelector("h4").textContent, top: bounds.top, bottom: bounds.bottom, trailingSpace: bounds.bottom - fields.bottom };
  }));
  if (layout.length !== 4) throw new Error("连接配置应完整显示四组卡片");
  for (let i = 0; i < layout.length; i++) {
    if (layout[i].trailingSpace > 20) throw new Error(`${layout[i].title} 被撑高，底部留白 ${layout[i].trailingSpace}px`);
    if (i > 0) {
      const gap = layout[i].top - layout[i - 1].bottom;
      if (gap < 0 || gap > 16) throw new Error(`${layout[i].title} 与上一张卡片间距异常：${gap}px`);
    }
  }
  return { operation: "完整 PLC 连接页四组卡片紧凑排列，无纵向换列产生的大段留白", passed: true, layout };
}
