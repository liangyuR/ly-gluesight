async (page) => {
  const initial = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("workspace_get", { id: "UI-FLYSHOT" }));
  const r = initial.layout, points = r.points;
  const holes = [];
  for (let j = 40; j < points.x.length; j += 110) {
    const a = (j + points.x.length - 1) % points.x.length, b = (j + 1) % points.x.length;
    const tx = points.x[b] - points.x[a], ty = points.y[b] - points.y[a], length = Math.hypot(tx, ty);
    holes.push([points.x[j] + 5 * ty / length, points.y[j] - 5 * tx / length]);
  }
  const frames = [];
  for (let k = 0; k < 4; k++) {
    await page.getByRole("button", { name: "选择帧 k" + (k + 1), exact: true }).click();
    await page.getByRole("button", { name: "取新样本", exact: true }).click();
    const svg = page.locator("svg.wp-gray-image.editable");
    await svg.waitFor();
    await page.getByRole("spinbutton", { name: "最低定位分数", exact: true }).fill("0.61");
    await page.getByText("本帧参数有未保存的修改", { exact: true }).waitFor();
    const [cx,cy] = r.shots[k].center, fov = r.shots[k].fov ?? r.fov;
    const [hx,hy] = [...holes].sort((a,b) => Math.hypot(a[0]-cx,a[1]-cy)-Math.hypot(b[0]-cx,b[1]-cy))[0];
    let nearest = 0;
    for (let j = 1; j < points.x.length; j++) if (Math.hypot(points.x[j]-hx,points.y[j]-hy) < Math.hypot(points.x[nearest]-hx,points.y[nearest]-hy)) nearest = j;
    const tx = (points.x[nearest]+hx)/2, ty = (points.y[nearest]+hy)/2;
    const w = Math.round(fov[0]/.08), h = Math.round(fov[1]/.08), size = 350;
    const x = Math.max(40,Math.min(w-size-40,Math.trunc((tx-cx+fov[0]/2)/.08)-175));
    const y = Math.max(40,Math.min(h-size-40,Math.trunc((ty-cy+fov[1]/2)/.08)-175));
    await svg.scrollIntoViewIfNeeded();
    const pointer = await svg.locator("g").first().evaluate((el, [x,y,size]) => {
      const matrix = el.getScreenCTM();
      return [[x,y],[x+size,y+size]].map(([x,y]) => { const p = new DOMPoint(x,y).matrixTransform(matrix); return { x:p.x, y:p.y }; });
    }, [x,y,size]);
    await page.mouse.move(pointer[0].x,pointer[0].y); await page.mouse.down();
    await page.mouse.move(pointer[1].x,pointer[1].y,{ steps:10 }); await page.mouse.up();
    await page.getByRole("button", { name: "试测当前帧", exact: true }).click();
    await page.getByText("当前冻结图像的试测已完成", { exact: true }).waitFor();
    const view = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("workspace_get", { id: "UI-FLYSHOT" }));
    const frame = view.workspace.frames[k];
    if (!frame.trial?.passed || frame.params.rect.some((v,i) => Math.abs(v-[x,y,size,size][i])>8)) throw new Error(JSON.stringify({ k, expected:[x,y,size,size], params:frame.params, trial:frame.trial && {passed:frame.trial.passed, coverage:frame.trial.coverage, reason:frame.trial.reason} }));
    await page.getByRole("button", { name: "保存本帧示教", exact: true }).click();
    await page.getByText("本帧示教已保存，发布前仍需整体验证", { exact: true }).waitFor();
    frames.push({ k, image:frame.image, params:frame.params, score:frame.trial.score, coverage:frame.trial.coverage, elapsedMs:frame.trial.elapsedMs });
  }
  const result = { operation:"四帧真实取样、模板指针拖动、LyFlow 原图定位与测量、逐帧保存", passed:true, frames, fixture:"按名义圆角矩形渲染的合成灰度图；不代表现场精度" };
  await page.evaluate(result => window.__uiOperations.checks.push(result), result);
  await page.screenshot({ path:"D:\\project\\ly-gluesight\\output\\playwright\\ui-regression\\flyshot-teaching.png", fullPage:true });
  return result;
}
