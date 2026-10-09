async (page) => {
  return await page.evaluate(() => {
    const result = window.__uiOperations;
    if (!result || result.checks.length < 3 || result.checks.some(c => !c.passed)) throw new Error("The basic UI checks have not completed");
    return { ...result, completedAt: new Date().toISOString(), scope: "隔离桌面实例的实际 UI 操作；详细覆盖与边界见 UI_OPERATIONS.md",
      measurementEvidence: "图像结论由实际原生算法计算；合成灰度测试图不作为现场准确率证据" };
  });
}
