import { expect, test } from "@playwright/test";

// §8.2 — the anonymous marketing landing page.
test("anonymous landing page shows the pitch, CTA, and Miniflux comparison", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toContainText(
    "Own the feed",
  );
  await expect(
    page.getByRole("link", { name: "Create your account" }).first(),
  ).toBeVisible();
  await expect(page.getByText("content: shared once")).toBeVisible();
  await expect(page.getByText("state: personal")).toBeVisible();
  await expect(page.getByText("Postgres is the queue")).toBeVisible();
  await expect(page.getByText("Rusty Feed vs Miniflux")).toBeVisible();
  // Optimized screenshots are lazy-loaded.
  await expect(page.locator("img[loading='lazy']").first()).toBeVisible();
});

test("the isolated demo starts in Rusty with organized showcase feeds", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Try the live demo" }).first().click();
  await expect(page).toHaveURL(/\/articles/);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "rusty");
  await expect(
    page.locator("#library-sidebar .folder-tree-node:not(.folder-tree-unfiled)").first(),
  ).toBeVisible();
  await expect(
    page.locator("#library-sidebar .sidebar-feed-drag-handle").first(),
  ).toBeVisible();
});

test("robots.txt disallows the app and points at the sitemap", async ({
  request,
}) => {
  const res = await request.get("/robots.txt");
  expect(res.status()).toBe(200);
  const body = await res.text();
  expect(body).toContain("Disallow: /admin");
  expect(body).toContain("Sitemap:");
});

async function register(page, suffix) {
  await page.goto("/register");
  await page.getByLabel("Email").fill(`wizard-${suffix}@example.test`);
  await page
    .getByLabel("Password (min 10 characters)")
    .fill("Str0ng-Test-Pass-9x");
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/welcome/);
}

// §8.4 — the first-run wizard live-previews with zero inline styles / no CSP
// violations, then persists and routes into the reader.
test("first-run wizard live-previews theme & typography without CSP violations", async ({
  page,
}, testInfo) => {
  const cspViolations = [];
  page.on("console", (msg) => {
    if (
      msg.type() === "error" &&
      /content security policy/i.test(msg.text())
    ) {
      cspViolations.push(msg.text());
    }
  });

  await register(page, `${Date.now()}-${testInfo.parallelIndex}`);

  // Selecting a theme swaps <html data-theme> live — no reload, no inline style.
  await page.getByRole("radio", { name: /Rusty/ }).check();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "rusty");
  await expect(page.locator("[data-theme-preview-name]")).toHaveText("Rusty");
  await expect(page.locator("[data-theme-live-preview]")).toHaveCSS("background-color", "rgb(255, 250, 246)");
  await page.getByRole("radio", { name: "Serif", exact: true }).check();
  await expect(page.locator("html")).toHaveAttribute(
    "data-font-family",
    "serif",
  );

  const interests = page.locator("[data-wizard-interest]");
  const firstInterest = interests.nth(0);
  const secondInterest = interests.nth(1);
  await firstInterest.check();
  const visibleSuggestion = page.locator("[data-wizard-feed]:visible input[name='feed_id']").first();
  const selectedFeedId = await visibleSuggestion.getAttribute("value");
  const selectedSuggestion = page.locator(`input[name='feed_id'][value='${selectedFeedId}']`);
  await selectedSuggestion.check();
  await firstInterest.uncheck();
  await secondInterest.check();
  await expect(selectedSuggestion).toBeChecked();
  await expect(selectedSuggestion.locator("xpath=..")).toBeHidden();
  await expect(page.locator("[data-wizard-selection-count]")).toContainText("1 selected");
  await page.getByRole("radio", { name: "Live", exact: true }).check();

  // Finish persists the selections and lands in the reader.
  await page.getByRole("button", { name: "Finish" }).click();
  await expect(page).toHaveURL(/\/articles/);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "rusty");

  expect(cspViolations, cspViolations.join("\n")).toHaveLength(0);
});
