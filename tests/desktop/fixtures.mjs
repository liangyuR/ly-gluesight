import { test as base, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdir, readFile, rm } from 'node:fs/promises';
import path from 'node:path';
import { instanceCdp, uiPage } from '../native/lib.mjs';
import { options } from './options.mjs';

function powershell(script, args = []) {
  return new Promise((resolve, reject) => {
    const child = spawn('pwsh', ['-NoProfile', '-File', path.resolve('scripts', script), ...args], {
      windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', chunk => { output += chunk; });
    const timer = setTimeout(() => { child.kill(); reject(new Error(`${script} 超时：${output.slice(-6000)}`)); }, 150000);
    child.on('error', error => { clearTimeout(timer); reject(error); });
    child.on('close', code => {
      clearTimeout(timer);
      if (code === 0) resolve(output);
      else reject(new Error(`${script} 失败（退出码 ${code}）：${output.slice(-6000)}`));
    });
  });
}

export const test = base.extend({
  desktop: async ({ playwright }, use, testInfo) => {
    if (!options.runId) throw new Error('请通过 pnpm test:ui:production 入口运行桌面测试。');
    const manifest = options.manifest;
    const traces = [];
    let owned, browser, context, page, segment = 0, originalFailure = false;
    const diagnostics = [];
    const saveTrace = async () => {
      if (!context) return;
      const trace = testInfo.outputPath(`trace-${segment}.zip`);
      try { await context.tracing.stop({ path: trace }); traces.push(trace); }
      catch (error) { diagnostics.push(`保存 trace 失败：${error.message}`); }
      context = null;
    };
    const stop = async () => {
      await saveTrace();
      const connected = browser;
      browser = null;
      try { if (connected) await connected.close(); }
      finally {
        if (owned) {
          await powershell('Stop-UiTestInstance.ps1', ['-Profile', options.profile, '-Instance', JSON.stringify(owned)]);
          owned = null;
        }
      }
    };
    const start = async (fresh) => {
      const args = ['-Profile', options.profile, '-DebugPort', String(options.debugPort), '-SkipBuild', '-RunId', options.runId];
      if (fresh) args.push('-ResetData');
      if (options.dataRoot) args.push('-DataRoot', options.dataRoot);
      try { await powershell('Start-UiTestInstance.ps1', args); }
      finally {
        try {
          const instance = JSON.parse((await readFile(manifest, 'utf8')).replace(/^\uFEFF/, ''));
          if (instance.runId === options.runId) owned = instance;
        } catch {}
      }
      if (!owned) throw new Error('启动后的实例清单不属于本次测试。');
      const cdp = await instanceCdp(manifest, undefined, id => id === `com.xyzrobotics.tujiaovision.${options.profile}-tests`);
      browser = await playwright.chromium.connectOverCDP(cdp);
      context = browser.contexts()[0];
      page = await uiPage(browser);
      segment++;
      await mkdir(testInfo.outputDir, { recursive: true });
      await context.tracing.start({ screenshots: true, snapshots: true, sources: true });
    };
    try {
      await start(options.resetData);
      await use({
        get page() { return page; }, manifest, output: options.output, options,
      });
    } catch (error) {
      originalFailure = true;
      throw error;
    } finally {
      const failed = originalFailure || testInfo.status !== testInfo.expectedStatus;
      if (failed && page && !page.isClosed()) {
        const screenshot = testInfo.outputPath('failure.png');
        try {
          await page.screenshot({ path: screenshot, fullPage: true, timeout: 5000 });
          await testInfo.attach('失败截图', { path: screenshot, contentType: 'image/png' });
        } catch (error) { diagnostics.push(`保存截图失败：${error.message}`); }
      }
      let cleanupError;
      try { await stop(); } catch (error) { cleanupError = error; diagnostics.push(`关闭实例失败：${error.message}`); }
      for (const file of traces) {
        try {
          if (failed || cleanupError) await testInfo.attach(path.basename(file), { path: file, contentType: 'application/zip' });
          else await rm(file, { force: true });
        } catch (error) { diagnostics.push(`保存附件失败：${error.message}`); }
      }
      for (const name of ['stdout.log', 'stderr.log']) {
        try { await testInfo.attach(name, { path: path.join(options.output, name), contentType: 'text/plain' }); }
        catch (error) { diagnostics.push(`附加 ${name} 失败：${error.message}`); }
      }
      if (diagnostics.length) await testInfo.attach('清理与证据采集', { body: diagnostics.join('\n'), contentType: 'text/plain' }).catch(() => {});
      if (cleanupError && !failed) throw cleanupError;
    }
  },
});

export { expect, options };
