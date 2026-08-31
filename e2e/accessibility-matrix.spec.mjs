import { expect, test } from "@playwright/test";

async function register(page, suffix) {
  await page.goto("/register");
  await page.getByLabel("Email").fill(`matrix-${suffix}@example.test`);
  await page.getByLabel("Password (min 10 characters)").fill("Str0ng-Test-Pass-9x");
  await page.getByRole("button", { name: "Create account" }).click();
  // New accounts land on the first-run wizard; skip it to reach the reader.
  await expect(page).toHaveURL(/\/welcome/);
  await page.getByRole("button", { name: "Skip for now" }).click();
  await expect(page).toHaveURL(/\/articles/);
}

const viewports = [
  { name: "desktop", width: 1440, height: 900 },
  { name: "tablet-portrait", width: 834, height: 1112 },
  { name: "tablet-landscape", width: 1112, height: 834 },
  { name: "mobile-portrait", width: 390, height: 844 },
  { name: "mobile-landscape", width: 844, height: 390 },
  { name: "narrow-mobile", width: 320, height: 568 },
];

test("authenticated shell remains keyboard-usable without overflow at supported viewports", async ({ page }, testInfo) => {
  await register(page, `${Date.now()}-${testInfo.parallelIndex}`);
  const adminProbe = await page.request.get("/admin");
  const paths = ["/articles", "/feeds", "/settings/preferences", "/settings/ai"];
  if (adminProbe.status() === 200) {
    paths.push("/admin", "/admin/users", "/admin/feeds", "/admin/catalog", "/admin/logs", "/admin/costs");
  }

  for (const viewport of viewports) {
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    for (const path of paths) {
      await page.goto(path);
      await expect(page.locator("#main-content")).toBeVisible();
      await expect(page.getByRole("link", { name: "Skip to content" })).toBeAttached();
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow, `${viewport.name} ${path} horizontal overflow`).toBeLessThanOrEqual(1);
      await page.keyboard.press("Home");
      await page.keyboard.press("Tab");
      await expect(page.getByRole("link", { name: "Skip to content" })).toBeFocused();
      await page.keyboard.press("Enter");
      await expect(page.locator("#main-content")).toBeFocused();
      await expect(page.locator("details.account-menu > summary")).toBeVisible();
      if (viewport.width <= 864) {
        const undersized = await page.locator("main button:visible, main .btn:visible, main input:not([type='checkbox']):not([type='radio']):visible, main select:visible, main textarea:visible").evaluateAll((elements) => elements.filter((element) => {
          const rect = element.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0 && rect.height < 40;
        }).map((element) => ({ tag: element.tagName, text: element.textContent?.trim(), height: element.getBoundingClientRect().height })));
        expect(undersized, `${viewport.name} ${path} undersized touch targets`).toEqual([]);
      }
    }
  }
});

test("mobile dialogs and wide operational data stay inside their viewport", async ({ page }, testInfo) => {
  await register(page, `mobile-surfaces-${Date.now()}-${testInfo.parallelIndex}`);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/feeds");
  const dialog = page.locator("dialog").first();
  if (await dialog.count()) {
    await dialog.evaluate((element) => element.showModal());
    const bounds = await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: innerWidth, height: innerHeight };
    });
    expect(bounds.left).toBeGreaterThanOrEqual(0);
    expect(bounds.right).toBeLessThanOrEqual(bounds.width + 1);
    expect(bounds.top).toBeGreaterThanOrEqual(0);
    expect(bounds.bottom).toBeLessThanOrEqual(bounds.height + 1);
  }

  const adminResponse = await page.goto("/admin/logs");
  if (adminResponse?.status() === 200) {
    const tableWraps = page.locator(".table-wrap");
    for (let index = 0; index < await tableWraps.count(); index += 1) {
      const dimensions = await tableWraps.nth(index).evaluate((element) => ({
        viewportWidth: element.getBoundingClientRect().width,
        contentWidth: element.scrollWidth,
        overflowX: getComputedStyle(element).overflowX,
      }));
      expect(dimensions.viewportWidth).toBeLessThanOrEqual(390);
      if (dimensions.contentWidth > dimensions.viewportWidth + 1) expect(dimensions.overflowX).toBe("auto");
    }
  }
});

test("reduced-motion preference disables animated transitions", async ({ page }, testInfo) => {
  await register(page, `motion-${Date.now()}-${testInfo.parallelIndex}`);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/articles");
  const duration = await page.locator(".sidebar").evaluate((element) => getComputedStyle(element).transitionDuration);
  expect(Number.parseFloat(duration)).toBeLessThanOrEqual(0.001);
});
