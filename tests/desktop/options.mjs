import { readFileSync } from 'node:fs';
import path from 'node:path';

const configured = process.env.GLUESIGHT_DESKTOP_RUN
  ? JSON.parse(readFileSync(process.env.GLUESIGHT_DESKTOP_RUN, 'utf8').replace(/^\uFEFF/, '')) : {};
export const options = {
  suite: 'production', profile: 'flow', debugPort: 9339, productionSuite: 'all',
  resetData: false,
  output: path.resolve('output/playwright/flow-regression'), ...configured,
};
