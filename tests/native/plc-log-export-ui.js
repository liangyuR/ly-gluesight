async (page) => {
  const startedAt = new Date().toISOString();
  await page.getByRole("navigation",{name:"操作导航"}).getByRole("link",{name:"通讯日志",exact:true}).click();
  await page.getByRole("button",{name:"清空筛选",exact:true}).click();
  await page.getByRole("combobox",{name:"日志点位",exact:true}).selectOption("p_result_code");
  await page.getByRole("heading",{name:"点位趋势 · 结果码",exact:true}).waitFor();
  await page.getByRole("button",{name:"实时跟随中",exact:true}).click();
  await page.getByRole("button",{name:"导出 CSV",exact:true}).click();
  await page.getByRole("button",{name:"导出 CSV",exact:true}).waitFor();
  if (await page.getByRole("alert").count()) throw new Error(await page.getByRole("alert").allTextContents());
  await page.getByRole("button",{name:"清空筛选",exact:true}).click();
  const result={operation:"通讯日志点位筛选、真实趋势、关闭实时跟随、导出 CSV 与清空筛选",startedAt,completedAt:new Date().toISOString(),downloadPrefix:"plc-log-",fileVerificationRequired:true};
  await page.screenshot({path:"output/playwright\\ui-regression\\plc-log-export.png",fullPage:true});
  // WebView2 可能直接写入系统下载目录而不发送 CDP download 事件。
  // 运行后核实该时间范围内的新 CSV 文件，并将文件证据合并到最终报告。
  return result;
}
