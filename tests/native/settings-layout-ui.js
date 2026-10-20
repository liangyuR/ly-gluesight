async (page) => {
  await page.getByRole("spinbutton", { name: "记录保留（天）", exact: true }).waitFor();
  await page.getByRole("spinbutton", { name: "离线原图最多保留（件）", exact: true }).waitFor();
  const layout = [];
  for (const name of ["检测节拍", "lyFlow 与帧录制"]) {
    const spacing = await page.getByRole("region", { name, exact: true }).evaluate(card => {
      const fieldset = card.querySelector("fieldset");
      const grid = fieldset.querySelector(".qs-grid").getBoundingClientRect();
      const note = fieldset.nextElementSibling.getBoundingClientRect();
      return { gap: note.top - grid.bottom, height: card.getBoundingClientRect().height };
    });
    if (spacing.gap < 0 || spacing.gap > 16) throw new Error(`${name} 表单到说明文字的间距异常：${spacing.gap}px`);
    layout.push({ name, ...spacing });
  }
  return { operation: "系统设置表单按内容确定高度，说明文字紧随表单", passed: true, layout };
}
