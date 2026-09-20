import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/browser',
  timeout: 30_000,
  fullyParallel: true,
  workers: 2,
  use: {
    headless: true,
    viewport: { width: 1180, height: 810 },
    launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
      : {},
    trace: 'retain-on-failure'
  },
  webServer: [
    { command: 'npm run dev -- --host 127.0.0.1 --port 15173', url: 'http://127.0.0.1:15173', reuseExistingServer: !process.env.CI },
    { command: 'npm run dev --prefix android -- --host 127.0.0.1 --port 15174', url: 'http://127.0.0.1:15174', reuseExistingServer: !process.env.CI },
    { command: 'npm run website:build && node tests/serve-website.mjs', url: 'http://127.0.0.1:15175', reuseExistingServer: !process.env.CI }
  ]
});
