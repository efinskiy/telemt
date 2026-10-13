import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './tests', workers: 1, timeout: 30000,
  use: { baseURL: 'http://127.0.0.1:5178', launchOptions: { executablePath: process.env.CHROME_PATH ?? (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : undefined) } },
  webServer: { command: 'npm run build && npx vite preview --host 127.0.0.1 --port 5178 --strictPort', url: 'http://127.0.0.1:5178', reuseExistingServer: false },
});
