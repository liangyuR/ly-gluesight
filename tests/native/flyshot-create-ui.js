async (page) => {
  await page.getByRole("button", { name: "复制配方 MTR-HSG-B", exact: true }).click();
  await page.getByRole("textbox", { name: "配方编号", exact: true }).fill("UI-FLYSHOT");
  await page.getByRole("textbox", { name: "配方名称", exact: true }).fill("离线 UI 飞拍验收");
  await page.getByRole("spinbutton", { name: "产品代码", exact: true }).fill("93");
  await page.getByRole("button", { name: "创建候选", exact: true }).click();
  await page.getByRole("heading", { name: "胶路与拍照规划", exact: true }).waitFor();
  return { url: page.url(), candidate: "UI-FLYSHOT" };
}
