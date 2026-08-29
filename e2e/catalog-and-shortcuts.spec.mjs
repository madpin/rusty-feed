import { expect, test } from "@playwright/test";

async function register(page, suffix) {
  await page.goto("/register");
  await page.getByLabel("Email").fill(`playwright-${suffix}@example.test`);
  await page.getByLabel("Password (min 8 characters)").fill("password123");
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/articles/);
}

test("a reader can browse the catalog and configure guarded shortcuts", async ({ page }, testInfo) => {
  await register(page, `${Date.now()}-${testInfo.parallelIndex}`);

  await page.goto("/catalog");
  await expect(page.getByRole("heading", { name: "Discover feeds" })).toBeVisible();
  await expect(page.getByText("Add your own feed URL")).toBeVisible();
  expect(await page.locator(".catalog-card").count()).toBeGreaterThanOrEqual(41);
  await expect(page.getByText("Go Blog")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Al Jazeera English" })).toBeVisible();

  const choices = page.locator("input[name='feed_id']:not(:disabled)");
  await choices.nth(0).check();
  await choices.nth(1).check();
  await page.getByRole("button", { name: "Subscribe to selected" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Subscribed to 2 source" })).toBeVisible();

  const adminResponse = await page.goto("/admin");
  expect(adminResponse?.status()).toBe(403);

  await page.goto("/settings/preferences");
  const nextArticle = page.locator("input[name='shortcut_next_article']");
  const previousArticle = page.locator("input[name='shortcut_previous_article']");
  await nextArticle.click();
  await page.keyboard.press("ArrowDown");
  await expect(nextArticle).toHaveValue("ArrowDown");

  await previousArticle.click();
  await page.keyboard.press("ArrowDown");
  await page.getByRole("button", { name: "Save preferences" }).click();
  await expect(previousArticle).toHaveJSProperty("validationMessage", "Each shortcut must be unique.");

  await nextArticle.click();
  await page.keyboard.press("Control+J");
  await expect(nextArticle).toHaveValue("ArrowDown");
  await expect(page.locator("#action-status")).toContainText("Ctrl, Alt, and Command are reserved.");

  await previousArticle.click();
  await page.keyboard.press("ArrowUp");
  await page.getByRole("button", { name: "Save preferences" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Preferences saved" })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("data-shortcut-next-article", "ArrowDown");

  await page.getByLabel("Enable keyboard shortcuts").uncheck();
  await page.getByRole("button", { name: "Save preferences" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-shortcuts-enabled", "false");
});
