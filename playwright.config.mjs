import { defineConfig } from "@playwright/test";

const baseURL = process.env.PLAYWRIGHT_BASE_URL || "http://127.0.0.1:8088";
const databaseURL = process.env.PLAYWRIGHT_DATABASE_URL || process.env.TEST_DATABASE_URL || "postgres://rustss:rustss@127.0.0.1:5435/rustss";
const redisURL = process.env.PLAYWRIGHT_REDIS_URL || process.env.TEST_REDIS_URL || "redis://127.0.0.1:6379";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["github"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  webServer: process.env.PLAYWRIGHT_BASE_URL
    ? undefined
    : {
        command: "cargo run --quiet -p rustss-server",
        url: `${baseURL}/api/health/ready`,
        reuseExistingServer: !process.env.CI,
        timeout: 120_000,
        env: {
          ...process.env,
          BIND_ADDR: "127.0.0.1:8088",
          DATABASE_URL: databaseURL,
          REDIS_URL: redisURL,
          SESSION_SECRET: "playwright-session-secret-must-be-at-least-32-characters",
          ENCRYPTION_SECRET: "playwright-encryption-secret-must-be-at-least-32-characters",
          EMBEDDING_PROVIDER: "local",
          EMBEDDING_MODEL: "hash-placeholder-v1",
          RUN_EMBEDDED_WORKER: "false",
          LOG_LEVEL: "warn",
        },
      },
});
