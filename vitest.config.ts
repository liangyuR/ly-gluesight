import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./tests/setup.ts"],
    include: ["tests/**/*.test.{ts,tsx}"],
    clearMocks: true,
    restoreMocks: true,
    maxWorkers: 2,
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: ["src/main.tsx", "src/vite-env.d.ts", "src/**/types.ts", "src/**/index.ts"],
      reporter: ["text", "html", "json-summary"],
      thresholds: {
        statements: 85,
        branches: 80,
        functions: 80,
        lines: 85,
        "src/features/workspace/context.tsx": { lines: 90, branches: 70 },
        "src/features/workflow/model.ts": { lines: 80, branches: 70 },
        "src/features/camera/components/FollowCalibPanel.tsx": { lines: 90, branches: 80 },
        "src/features/workspace/OverviewPage.tsx": { lines: 90, branches: 80 },
      },
    },
  },
});
