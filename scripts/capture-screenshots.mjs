// Capture product screenshots for the README/docs against a running RustSS.
//
// Usage:
//   1. Start RustSS (see docs/screenshots/README.md), e.g. on http://127.0.0.1:8088
//   2. node scripts/capture-screenshots.mjs
//
// It logs in (or registers) a showcase account, subscribes it to the discovery
// magazine so the library is populated, and writes PNGs to docs/screenshots/.
// Admin pages are captured only when the account has the admin role (promote it
// once with: UPDATE users SET role='admin' WHERE email='<SHOT_EMAIL>').
import { chromium } from "@playwright/test";
import { mkdir } from "node:fs/promises";

const baseURL = process.env.SHOT_BASE_URL || "http://127.0.0.1:8088";
const outDir = process.env.SHOT_OUT_DIR || "docs/screenshots";
const email = process.env.SHOT_EMAIL || "showcase@rustss.local";
const password = process.env.SHOT_PASSWORD || "showcase-password-123";

await mkdir(outDir, { recursive: true });
const browser = await chromium.launch();
const context = await browser.newContext({
  viewport: { width: 1440, height: 900 },
  deviceScaleFactor: 2,
  colorScheme: "light",
});
const page = await context.newPage();

async function shot(name, { fullPage = false } = {}) {
  try {
    await page.waitForLoadState("networkidle", { timeout: 8000 }).catch(() => {});
    await page.screenshot({ path: `${outDir}/${name}.png`, fullPage });
    console.log("captured", name);
  } catch (error) {
    console.warn("skip", name, String(error).split("\n")[0]);
  }
}

async function authenticate() {
  await page.goto(`${baseURL}/register`);
  await page.locator('input[name="email"]').first().fill(email);
  await page.locator('input[name="password"]').first().fill(password);
  await page.getByRole("button", { name: "Create account" }).click().catch(() => {});
  await page.waitForTimeout(1200);
  if (page.url().includes("/register") || page.url().includes("/login")) {
    await page.goto(`${baseURL}/login`);
    await page.locator('input[name="email"]').first().fill(email);
    await page.locator('input[name="password"]').first().fill(password);
    await page.getByRole("button", { name: "Sign in" }).click();
    await page.waitForTimeout(1200);
  }
}

async function isAdmin() {
  const response = await page.request.get(`${baseURL}/admin`);
  return response.ok();
}

await authenticate();

// Discover magazine — show the filter-aware "select all" engaged before subscribing.
await page.goto(`${baseURL}/catalog`);
const selectAll = page.locator("[data-catalog-select-all]");
if (await selectAll.count()) {
  await selectAll.check().catch(() => {});
  await page.waitForTimeout(300);
}
await shot("discover");

// Subscribe so the reading list is populated with real, already-ingested articles.
const subscribe = page.getByRole("button", { name: /Subscribe to selected/ });
if (await subscribe.count()) {
  await subscribe.click().catch(() => {});
  await page.waitForTimeout(2500);
}

// Library with the sidebar unread counts.
await page.goto(`${baseURL}/articles`);
await page.waitForTimeout(600);
await shot("library");

// Standalone article reader.
const firstArticle = page.locator("a.article-link").first();
const href = (await firstArticle.count()) ? await firstArticle.getAttribute("href") : null;
if (href) {
  await page.goto(`${baseURL}${href}`);
  await page.waitForTimeout(600);
  await shot("article");
}

// Feeds management (bulk toolbar incl. the new content action).
await page.goto(`${baseURL}/feeds`);
await shot("feeds");

// AI providers and integrations settings.
await page.goto(`${baseURL}/settings/ai`);
await shot("ai-providers");
await page.goto(`${baseURL}/settings/integrations`);
await shot("integrations");

// Admin surfaces (only if this account is an admin).
if (await isAdmin()) {
  await page.goto(`${baseURL}/admin`);
  await shot("admin-overview");
  await page.goto(`${baseURL}/admin/feeds`);
  await shot("admin-feeds");
  await page.goto(`${baseURL}/admin/logs`);
  await shot("admin-logs");
} else {
  console.log("account is not admin; skipping admin screenshots");
}

await browser.close();
console.log("done");
