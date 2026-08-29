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
  expect(await page.locator(".catalog-card").count()).toBeGreaterThanOrEqual(97);
  await expect(page.getByText("Go Blog")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Al Jazeera English" })).toBeVisible();
  await page.getByLabel("Category").selectOption("Culture & Arts");
  await expect(page).toHaveURL(/category=Culture/);
  await expect(page.getByRole("heading", { name: "Smithsonian Arts and Culture" })).toBeVisible();
  await page.goto("/catalog");
  await page.getByLabel("Country").selectOption("Brazil");
  await expect(page).toHaveURL(/country=Brazil/);
  await expect(page.getByRole("heading", { name: "Agência Brasil — Últimas" })).toBeVisible();
  await page.goto("/catalog");
  const goBlogCard = page.locator(".catalog-card").filter({
    has: page.getByRole("heading", { name: "Go Blog" }),
  });
  await goBlogCard.getByRole("link", { name: "Preview", exact: true }).click();
  await expect(page).toHaveURL(/\/catalog\/[^/]+\/preview/);
  await expect(page.getByRole("heading", { name: "Preview Go Blog" })).toBeVisible();
  await expect(page.getByRole("link", { name: "RSS preview" })).toHaveAttribute("aria-current", "page");
  await page.getByRole("link", { name: "Processed availability" }).click();
  await expect(page).toHaveURL(/view=processed/);
  await expect(page.getByText("Processed preview is an availability report")).toBeVisible();
  await expect(page.getByRole("link", { name: "Processed availability" })).toHaveAttribute("aria-current", "page");
  await page.goto("/catalog");

  const choices = page.locator("input[name='feed_id']:not(:disabled)");
  await choices.nth(0).check();
  await choices.nth(1).check();
  await page.getByRole("button", { name: "Subscribe to selected" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Subscribed to 2 source" })).toBeVisible();

  await page.goto("/feeds");
  await expect(page.getByText(/Last updated:/).first()).toBeVisible();
  await page.getByLabel("Folder name").fill("Browser folder");
  await page.getByRole("button", { name: "Create folder" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Browser folder" })).toBeVisible();
  await page.getByRole("button", { name: "Organize", exact: true }).first().click();
  const organization = page.locator("[data-feed-organizer]");
  await expect(organization).toBeVisible();
  await organization.locator("[data-folder-search]").fill("Browser");
  await organization.locator("[data-folder-select]").selectOption({ label: "Browser folder" });
  await organization.locator("[data-feed-organizer-tags]").fill("browser, e2e");
  await organization.getByRole("button", { name: "Save organization" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Feed organization saved" })).toBeVisible();
  await page.getByRole("button", { name: "Organize", exact: true }).first().click();
  await expect(organization.locator("[data-tag-select]")).toHaveJSProperty("multiple", true);
  await organization.locator("[data-tag-search]").fill("browser");
  await expect(organization.locator("option[value='browser']")).toHaveJSProperty("hidden", false);
  await expect(organization.locator("option[value='e2e']")).toHaveJSProperty("hidden", true);
  await organization.getByRole("button", { name: "Cancel" }).click();
  await page.goto("/articles");
  const sidebar = page.locator("#library-sidebar");
  await expect(page.getByRole("heading", { name: "Most recent" })).toBeVisible();
  await expect(page).toHaveTitle("Most recent · RustSS");
  await sidebar.getByRole("link", { name: "Unread only", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Unread only" })).toBeVisible();
  await sidebar.getByRole("link", { name: "Personalized", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Personalized" })).toBeVisible();
  await page.goto("/articles");
  await expect(sidebar.locator(".folder-tree-feeds [data-feed-nav]").first()).toBeVisible();
  expect(await sidebar.locator(".folder-tree-feeds [data-feed-nav]").count()).toBeGreaterThanOrEqual(2);
  await expect(sidebar.locator("details.folder-tree-node")).toHaveCount(0);
  await sidebar.getByRole("link", { name: "Browser folder", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Folder: Browser folder" })).toBeVisible();
  await expect(sidebar.getByRole("link", { name: "Unfiled feeds" })).toBeVisible();
  await sidebar.getByRole("link", { name: "Unfiled feeds" }).click();
  await expect(page.getByRole("heading", { name: "Unfiled" })).toBeVisible();
  await sidebar.getByRole("link", { name: "Browser folder", exact: true }).click();
  await sidebar.getByRole("link", { name: "browser", exact: true }).click();
  await expect(page).toHaveURL(/tag=browser/);
  await expect(page.getByRole("heading", { name: "Browser folder · #browser" })).toBeVisible();

  const adminResponse = await page.goto("/admin");
  expect(adminResponse?.status()).toBe(403);

  await page.goto("/settings/preferences");
  const nextArticle = page.locator("input[name='shortcut_next_article']");
  const previousArticle = page.locator("input[name='shortcut_previous_article']");
  await expect(nextArticle).toHaveValue("ArrowDown");
  await expect(previousArticle).toHaveValue("ArrowUp");
  await expect(page.locator("input[name='shortcut_next_feed']")).toHaveValue("ArrowRight");
  await expect(page.locator("input[name='shortcut_previous_feed']")).toHaveValue("ArrowLeft");
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

  await page.goto("/articles");
  const expandedSidebarWidth = await sidebar.evaluate((element) => element.getBoundingClientRect().width);
  const sidebarToggle = page.locator("[data-sidebar-toggle]").first();
  await sidebarToggle.click();
  await expect(page.locator("html")).toHaveAttribute("data-sidebar-collapsed", "true");
  expect(await sidebar.evaluate((element) => element.getBoundingClientRect().width)).toBeLessThan(expandedSidebarWidth);
  await expect(page.locator("#action-status")).toContainText("Sidebar reduced.");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-sidebar-collapsed", "true");
  await page.locator("[data-sidebar-toggle]").first().click();
  await expect(page.locator("html")).toHaveAttribute("data-sidebar-collapsed", "false");
});

test("live view defers and clears the publisher frame", async ({ page }) => {
  await page.goto("/login");
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  await page.setContent(`
    <article>
      <button type="button" data-live-view-toggle aria-controls="live-view-test" aria-expanded="false">Live view</button>
      <div data-reader-view-content>Saved reader content</div>
      <section id="live-view-test" data-live-view hidden>
        <iframe data-live-view-frame data-live-src="https://example.invalid/live"></iframe>
      </section>
    </article>
    <div id="action-status" role="status"></div>
  `);
  await page.addScriptTag({ url: scriptUrl });

  const toggle = page.locator("[data-live-view-toggle]");
  const reader = page.locator("[data-reader-view-content]");
  const live = page.locator("[data-live-view]");
  const frame = page.locator("[data-live-view-frame]");
  await expect(frame).not.toHaveAttribute("src");

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(toggle).toHaveText("Reader view");
  await expect(reader).toBeHidden();
  await expect(live).toBeVisible();
  await expect(frame).toHaveAttribute("src", "https://example.invalid/live");

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(toggle).toHaveText("Live view");
  await expect(reader).toBeVisible();
  await expect(live).toBeHidden();
  await expect(frame).not.toHaveAttribute("src");
});

test("side-panel cards and catalog cards support direct selection", async ({ page }) => {
  await page.goto("/login");
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  await page.setContent(`
    <div class="reading-workspace">
      <ul class="article-list" data-reading-mode="side-panel" data-reading-panel="#reading-pane">
        <li class="article-card" id="card-one">
          <h3><a class="article-link" href="/articles/one">Article one</a></h3>
          <p class="excerpt">Open from this card surface.</p>
          <div class="actions"><button type="button">Leave this action alone</button></div>
        </li>
      </ul>
      <aside class="reading-pane" id="reading-pane" style="display:block"></aside>
    </div>
    <article class="catalog-card" data-catalog-selectable>
      <p class="catalog-description">Double click this source to select it.</p>
      <a href="/catalog/one/preview">Preview</a>
      <label><input type="checkbox" name="feed_id" value="one" /> Subscribe</label>
    </article>
    <form id="bulk-feed-actions">
      <input type="checkbox" data-bulk-select-all />
      <span data-bulk-selection-count></span>
      <button type="button" data-bulk-action>Move selected</button>
    </form>
    <input type="checkbox" data-feed-select form="bulk-feed-actions" />
    <input type="checkbox" data-feed-select form="bulk-feed-actions" />
    <div id="action-status" role="status"></div>
  `);
  await page.addScriptTag({
    content: `
      window.__panelRequests = 0;
      window.fetch = function () {
        window.__panelRequests += 1;
        return Promise.resolve({
          ok: true,
          text: function () {
            return Promise.resolve('<article data-panel-article="one" data-panel-read="false" data-panel-bookmarked="false"><h2>Article opened in panel</h2></article>');
          }
        });
      };
    `,
  });
  await page.addScriptTag({ url: scriptUrl });

  const card = page.locator("#card-one");
  await card.locator(".excerpt").click();
  await expect(page.locator("#reading-pane")).toContainText("Article opened in panel");
  await expect(card).toHaveClass(/selected/);
  expect(await page.evaluate(() => window.__panelRequests)).toBe(1);

  await card.getByRole("button", { name: "Leave this action alone" }).click();
  expect(await page.evaluate(() => window.__panelRequests)).toBe(1);

  const catalogCard = page.locator("[data-catalog-selectable]");
  await catalogCard.locator(".catalog-description").dblclick();
  await expect(catalogCard.locator("input[name='feed_id']")).toBeChecked();

  const bulkAction = page.locator("[data-bulk-action]");
  await expect(bulkAction).toBeDisabled();
  const bulkInputs = page.locator("[data-feed-select]");
  await bulkInputs.nth(0).check();
  await expect(page.locator("[data-bulk-selection-count]")).toHaveText("1 feed selected.");
  await expect(bulkAction).toBeEnabled();
  await page.locator("[data-bulk-select-all]").check();
  await expect(page.locator("[data-bulk-selection-count]")).toHaveText("2 feeds selected.");
});

test("dragging a feed onto a folder submits only its folder move", async ({ page }) => {
  await page.goto("/login");
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  await page.setContent(`
    <button type="button" data-feed-drag data-feed-id="feed-one" draggable="true">Drag feed</button>
    <div data-feed-drop data-folder-id="folder-one" data-folder-label="Folder one">Folder one</div>
    <div id="action-status" role="status"></div>
  `);
  await page.addScriptTag({
    content: `
      document.documentElement.setAttribute("data-csrf-token", "csrf-token");
      window.__moveRequest = null;
      window.setTimeout = function () { return 0; };
      window.fetch = function (url, options) {
        window.__moveRequest = { url: url, body: String(options.body) };
        return Promise.resolve({ ok: true });
      };
    `,
  });
  await page.addScriptTag({ url: scriptUrl });
  await page.evaluate(() => {
    const transfer = new DataTransfer();
    const drag = document.querySelector("[data-feed-drag]");
    const target = document.querySelector("[data-feed-drop]");
    drag.dispatchEvent(new DragEvent("dragstart", { bubbles: true, dataTransfer: transfer }));
    target.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: transfer }));
    target.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: transfer }));
  });
  await expect.poll(() => page.evaluate(() => window.__moveRequest?.url)).toBe("/feeds/feed-one/folder");
  await expect.poll(() => page.evaluate(() => window.__moveRequest?.body)).toContain("csrf_token=csrf-token");
  await expect.poll(() => page.evaluate(() => window.__moveRequest?.body)).toContain("folder_id=folder-one");
});

test.describe("catalog preview without JavaScript", () => {
  test.use({ javaScriptEnabled: false });

  test("uses normal links and forms", async ({ page }, testInfo) => {
    await register(page, `no-js-${Date.now()}-${testInfo.parallelIndex}`);
    await page.goto("/catalog");
    const goBlogCard = page.locator(".catalog-card").filter({
      has: page.getByRole("heading", { name: "Go Blog" }),
    });
    await goBlogCard.getByRole("link", { name: "Preview", exact: true }).click();
    await expect(page.getByRole("heading", { name: "Preview Go Blog" })).toBeVisible();
    await page.getByRole("link", { name: "Processed availability" }).click();
    await expect(page).toHaveURL(/view=processed/);
    await expect(page.getByText("Processed preview is an availability report")).toBeVisible();
  });
});
