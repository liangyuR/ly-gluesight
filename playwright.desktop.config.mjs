import { defineConfig } from '@playwright/test';
import { options } from './tests/desktop/options.mjs';
import path from 'node:path';

const runner = path.join(options.output, 'runner');
export default defineConfig({
  testDir: './tests/desktop',
  testMatch: '**/*.spec.mjs',
  timeout: 15 * 60 * 1000,
  workers: 1,
  fullyParallel: false,
  retries: 0,
  forbidOnly: !!process.env.CI,
  outputDir: path.join(runner, 'test-results'),
  reporter: [
    ['list'],
    ['html', { outputFolder: path.join(runner, 'html'), open: 'never' }],
    ['junit', { outputFile: path.join(runner, 'junit.xml') }],
  ],
  projects: [{ name: 'production', testMatch: 'production.spec.mjs' }],
});
