import { expect, test } from "@playwright/test";

async function register(page, suffix) {
  await page.goto("/register");
  await page.getByLabel("Email").fill(`playwright-roundups-${suffix}@example.test`);
  await page.getByLabel("Password (min 10 characters)").fill("Str0ng-Test-Pass-9x");
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/welcome/);
  await page.getByRole("button", { name: "Skip for now" }).click();
  await expect(page).toHaveURL(/\/articles/);
}

async function addFeed(page, run) {
  await page.goto("/feeds");
  const document = `<?xml version="1.0"?>
    <opml version="2.0"><head><title>Roundup browser feeds</title></head><body>
      <outline text="Coverage"><outline text="Browser Roundup Feed" xmlUrl="https://roundups-${run}.example/feed.xml" />
    </outline></body></opml>`;
  await page.getByLabel("OPML file").setInputFiles({
    name: "roundups.opml",
    mimeType: "text/x-opml",
    buffer: Buffer.from(document),
  });
  await page.getByRole("button", { name: "Preview import" }).click();
  await page.getByRole("button", { name: "Import selected" }).click();
  await expect(page.getByRole("heading", { name: "Import complete" })).toBeVisible();
}

test("a reader can create and manage a Roundup schedule and browse history in light and dark themes", async ({ page }, testInfo) => {
  const run = `${Date.now()}-${testInfo.parallelIndex}`;
  await register(page, run);
  await addFeed(page, run);

  await page.goto("/roundups");
  await expect(page.getByRole("heading", { name: "Roundups", exact: true })).toBeVisible();
  await expect(page.getByText("No repeating Roundups yet.")).toBeVisible();
  await page.getByLabel("Feed or Folder").selectOption({ label: "Browser Roundup Feed" });
  await page.locator("input[name='period'][value='day']").check();
  await page.getByLabel("Repeat on a schedule").check();
  await expect(page.getByLabel("Daily time")).toBeVisible();
  await page.getByLabel("Daily time").fill("07:00");
  await page.getByLabel("Email me when the Roundup is ready").check();
  await page.getByRole("button", { name: "Create Roundup" }).click();
  await expect(page).toHaveURL(/notice=created/);

  const schedule = page.locator(".roundup-schedule-row").filter({ hasText: "Browser Roundup Feed" });
  await expect(schedule).toContainText("Active");
  await expect(schedule).toContainText("Every day at 07:00");
  await expect(schedule).toContainText("Email on");
  await expect(schedule).toContainText("Next run");
  await schedule.getByRole("link", { name: "Edit schedule for Browser Roundup Feed", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Edit repeating Roundup" })).toBeVisible();
  await page.getByLabel("Daily time").fill("08:00");
  await page.getByLabel("Email me when the Roundup is ready").uncheck();
  await page.getByRole("button", { name: "Save schedule" }).click();
  await expect(page).toHaveURL(/notice=updated/);
  await expect(schedule).toContainText("Every day at 08:00");
  await expect(schedule).toContainText("Email off");
  await schedule.getByRole("button", { name: "Pause", exact: true }).click();
  await expect(schedule).toContainText("Paused");
  await schedule.getByRole("button", { name: "Resume", exact: true }).click();
  await expect(schedule).toContainText("Active");

  await page.goto("/roundups");
  await page.getByLabel("Feed or Folder").selectOption({ label: "Browser Roundup Feed" });
  await page.getByRole("button", { name: "Create Roundup" }).click();
  await expect(page.getByText(/at least 3 articles/)).toBeVisible();
  await page.goto("/roundups");
  await expect(page.getByRole("heading", { name: "History", exact: true })).toBeVisible();
  const history = page.locator(".roundup-history-row").filter({ hasText: "Browser Roundup Feed" }).first();
  await expect(history).toContainText("Skipped");
  await expect(history).toContainText("Fewer than 3 articles");

  await page.goto("/settings/preferences");
  await page.getByRole("radio", { name: /Light/ }).check();
  await page.getByRole("button", { name: "Save preferences" }).click();
  await page.goto("/roundups");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator(".roundup-create")).toBeVisible();
  expect(await page.locator(".roundup-create").evaluate((element) => getComputedStyle(element).backgroundColor)).not.toBe("rgba(0, 0, 0, 0)");

  await page.goto("/settings/preferences");
  await page.getByRole("radio", { name: /Dark/ }).check();
  await page.getByRole("button", { name: "Save preferences" }).click();
  await page.goto("/roundups");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.locator(".roundup-create")).toBeVisible();
  expect(await page.locator(".roundup-create").evaluate((element) => getComputedStyle(element).backgroundColor)).not.toBe("rgba(0, 0, 0, 0)");

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/roundups");
  await expect(schedule).toBeVisible();
  await expect(schedule.getByRole("link", { name: "Edit schedule for Browser Roundup Feed", exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
});
