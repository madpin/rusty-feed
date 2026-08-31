import { expect, test } from "@playwright/test";

// §8.2 — the anonymous marketing landing page.
test("anonymous landing page shows the pitch, CTA, and Miniflux comparison", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toContainText(
    "understand",
  );
  await expect(
    page.getByRole("link", { name: "Create free account" }).first(),
  ).toBeVisible();
  await expect(page.getByText("RustSS vs Miniflux")).toBeVisible();
  // Optimized screenshots are lazy-loaded.
  await expect(page.locator("img[loading='lazy']").first()).toBeVisible();
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
  await page.getByRole("radio", { name: /Ocean/ }).check();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "ocean");
  await page.getByRole("radio", { name: "Serif", exact: true }).check();
  await expect(page.locator("html")).toHaveAttribute(
    "data-font-family",
    "serif",
  );

  // Finish persists the selections and lands in the reader.
  await page.getByRole("button", { name: "Finish" }).click();
  await expect(page).toHaveURL(/\/articles/);

  expect(cspViolations, cspViolations.join("\n")).toHaveLength(0);
});
