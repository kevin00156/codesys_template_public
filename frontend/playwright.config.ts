import { defineConfig, devices } from '@playwright/test'

// E2E smoke 測試：vite dev server + 全 mock（page.route / routeWebSocket），
// 不依賴真後端 / PLC。tests/ 不在 vite build 的模組圖內（入口是 index.html）。
const PORT = 5273

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  reporter: 'list',
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'on-first-retry',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
  webServer: {
    // dev 而非 preview：免 build、啟動快且穩定；proxy 目標雖指向 PLC 主機，
    // 但所有 /api、/ws 都被測試端攔截，不會真的打出去。
    command: `npm run dev -- --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
})
