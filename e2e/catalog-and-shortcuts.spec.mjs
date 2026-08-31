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
  expect(await page.locator(".catalog-card").count()).toBeGreaterThanOrEqual(200);
  await expect(page.getByRole("heading", { name: "Hugging Face Blog" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Folha de S.Paulo (Em Cima da Hora)" })).toBeVisible();
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
  const browserFolder = sidebar.locator(".folder-tree-node").filter({ hasText: "Browser folder" });
  const folderToggle = browserFolder.locator("[data-folder-collapse]");
  await folderToggle.click();
  await expect(folderToggle).toHaveAttribute("aria-expanded", "false");
  await expect(browserFolder.locator(".folder-tree-feeds")).toBeHidden();
  await page.reload();
  await expect(folderToggle).toHaveAttribute("aria-expanded", "false");
  await expect(browserFolder.locator(".folder-tree-feeds")).toBeHidden();
  await folderToggle.click();
  await sidebar.getByRole("button", { name: "Quick feed tools" }).click();
  const contentButton = sidebar.locator("[data-feed-content-trigger]").first();
  await expect(contentButton).toBeVisible();
  await contentButton.click();
  const contentDialog = page.locator("[data-content-settings-dialog]");
  await expect(contentDialog).toBeVisible();
  await contentDialog.getByLabel("Public article page").check();
  await contentDialog.getByLabel("Public-page lifetime").selectOption("30");
  await contentDialog.getByRole("button", { name: "Save content settings" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Content settings saved" })).toBeVisible();
  await sidebar.locator("[data-feed-tags-trigger]").first().click();
  const tagsDialog = page.locator("[data-tags-settings-dialog]");
  await expect(tagsDialog).toBeVisible();
  await tagsDialog.getByLabel("Tags").fill("browser, quick-tools");
  await tagsDialog.getByRole("button", { name: "Save tags" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Tags saved" })).toBeVisible();
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
  await expect(page.getByRole("group", { name: "Content privacy" })).toContainText("balanced");
  await expect(page.getByLabel("Show how many tracking images RustSS removed from each article")).toBeChecked();
  const mediaDelivery = page.getByLabel("Media delivery");
  await expect(mediaDelivery).toHaveValue("proxy");
  await mediaDelivery.selectOption("mixed-content");
  const searchMode = page.getByLabel("Default search mode");
  const searchText = page.getByLabel("Text matching");
  const searchWeight = page.getByLabel("Meaning weight in hybrid mode (%)");
  await expect(searchMode).toHaveValue("instance");
  await expect(searchText).toHaveValue("instance");
  await expect(searchWeight).toHaveValue("");
  await searchMode.selectOption("keyword");
  await searchText.selectOption("english");
  await searchWeight.fill("35");
  await expect(page.getByLabel("Remote media loading")).toHaveValue("on-demand");
  await expect(page.getByLabel("Audio players")).toBeChecked();
  await expect(page.getByLabel("Video players")).toBeChecked();
  await expect(page.getByLabel("Image previews")).toBeChecked();
  await expect(page.getByRole("group", { name: "YouTube videos" })).toContainText("youtube-nocookie.com");
  const youtubeMode = page.getByLabel("Player behavior");
  const youtubeCaptions = page.getByLabel("Captions");
  const youtubeAutoplay = page.getByLabel("Start playback after I choose Load player");
  await expect(youtubeMode).toHaveValue("privacy");
  await expect(youtubeCaptions).toHaveValue("default");
  await expect(youtubeAutoplay).not.toBeChecked();
  await youtubeCaptions.selectOption("on");
  await youtubeAutoplay.check();
  const touchGestures = page.getByLabel("Swipe horizontally to move between articles");
  await expect(touchGestures).not.toBeChecked();
  await touchGestures.check();
  await page.getByLabel("Sensitivity").selectOption("high");
  await page.getByLabel("Direction").selectOption("reversed");
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
  await expect(mediaDelivery).toHaveValue("mixed-content");
  await expect(searchMode).toHaveValue("keyword");
  await expect(searchText).toHaveValue("english");
  await expect(searchWeight).toHaveValue("35");
  await expect(youtubeMode).toHaveValue("privacy");
  await expect(youtubeCaptions).toHaveValue("on");
  await expect(youtubeAutoplay).toBeChecked();
  await expect(page.locator("html")).toHaveAttribute("data-shortcut-next-article", "ArrowDown");
  await expect(page.locator("html")).toHaveAttribute("data-touch-gestures-enabled", "true");
  await expect(page.locator("html")).toHaveAttribute("data-touch-gesture-sensitivity", "high");
  await expect(page.locator("html")).toHaveAttribute("data-touch-gesture-direction", "reversed");

  await page.getByLabel("Enable keyboard shortcuts").uncheck();
  await page.getByRole("button", { name: "Save preferences" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-shortcuts-enabled", "false");

  await page.goto("/search");
  await expect(page.getByRole("heading", { name: "Search", exact: true })).toBeVisible();
  await expect(page.getByLabel("Search with")).toHaveValue("preferred");
  await expect(page.getByText("Current settings: keyword, english text", { exact: false })).toBeVisible();
  await expect(page.getByText('Exact search supports "quoted phrases"', { exact: false })).toBeVisible();

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

test("OPML import previews folders and selected subscriptions before writing", async ({ page }, testInfo) => {
  await register(page, `opml-${Date.now()}-${testInfo.parallelIndex}`);
  await page.goto("/feeds");
  const run = `${Date.now()}-${testInfo.parallelIndex}`;
  const firstUrl = `https://opml-${run}.example/first.xml`;
  const secondUrl = `https://opml-${run}.example/second.xml`;
  const document = `<?xml version="1.0"?>
    <opml version="2.0"><head><title>Browser OPML</title></head><body>
      <outline text="Work"><outline text="News">
        <outline text="First source" xmlUrl="${firstUrl}" />
      </outline></outline>
      <outline text="Personal"><outline text="News">
        <outline text="Second source" xmlUrl="${secondUrl}" />
      </outline></outline>
    </body></opml>`;
  await page.getByLabel("OPML file").setInputFiles({
    name: "subscriptions.opml",
    mimeType: "text/x-opml",
    buffer: Buffer.from(document),
  });
  await page.getByRole("button", { name: "Preview import" }).click();
  await expect(page.getByRole("heading", { name: "Preview Browser OPML" })).toBeVisible();
  await expect(page.getByText("2 new · 0 already subscribed")).toBeVisible();
  await expect(page.getByText("2 selected.")).toBeVisible();
  await expect(page.getByText("Work / News", { exact: false })).toBeVisible();
  await expect(page.getByText("Personal / News", { exact: false })).toBeVisible();

  await page.getByRole("button", { name: "Select none" }).click();
  await expect(page.getByText("0 selected.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Import selected" })).toBeDisabled();
  await page.getByRole("button", { name: "Select all" }).click();
  const personalGroup = page.locator("fieldset.opml-group").filter({ hasText: "Personal / News" });
  await personalGroup.locator("[data-opml-group-toggle]").uncheck();
  await expect(page.getByText("1 selected.")).toBeVisible();
  await page.getByRole("button", { name: "Import selected" }).click();
  await expect(page.getByRole("heading", { name: "Import complete" })).toBeVisible();
  await expect(page.getByText("RustSS processed 1 selected subscription")).toBeVisible();

  await page.getByRole("link", { name: "Back to feeds" }).click();
  await expect(page.getByText("First source")).toBeVisible();
  await expect(page.getByText("Second source")).toHaveCount(0);
  const downloadPromise = page.waitForEvent("download");
  await page.getByRole("link", { name: "Export OPML" }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toMatch(/^rustss-subscriptions-.*\.opml$/);
});

test("live view defers and clears the publisher frame", async ({ page }) => {
  await page.goto("/login");
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  await page.route("**/test-live-ticket", async (route) => {
    await route.fulfill({ status: 200, contentType: "text/plain", body: "https://example.invalid/live" });
  });
  await page.setContent(`
    <article>
      <button type="button" data-live-view-toggle aria-controls="live-view-test" aria-expanded="false">Live view</button>
      <div data-reader-view-content>Saved reader content</div>
      <section id="live-view-test" data-live-view hidden>
        <iframe data-live-view-frame data-live-ticket-url="/test-live-ticket"></iframe>
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

test("attachment players contact the media host only after consent", async ({ page }) => {
  await page.goto("/login");
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  let mediaRequests = 0;
  await page.route("https://media.example.test/episode.mp3", async (route) => {
    mediaRequests += 1;
    await route.fulfill({ status: 200, contentType: "audio/mpeg", body: "" });
  });
  await page.setContent(`
    <div class="enclosure-loader" data-enclosure-loader>
      <button type="button" data-enclosure-load data-enclosure-kind="audio">Load audio player</button>
      <span>Contacts the media host only after you choose Load.</span>
      <audio controls preload="none" data-enclosure-player data-src="https://media.example.test/episode.mp3" hidden></audio>
    </div>
    <div id="action-status" role="status"></div>
  `);
  await page.addScriptTag({ url: scriptUrl });

  const player = page.locator("[data-enclosure-player]");
  await expect(player).not.toHaveAttribute("src");
  await expect(player).not.toHaveAttribute("autoplay");
  expect(mediaRequests).toBe(0);

  await page.getByRole("button", { name: "Load audio player" }).click();
  await expect(player).toHaveAttribute("src", "https://media.example.test/episode.mp3");
  await expect(player).toBeVisible();
  await expect(page.locator("#action-status")).toContainText("audio loaded");
  await expect.poll(() => mediaRequests).toBeGreaterThan(0);
});

test("YouTube uses the privacy origin only after explicit consent", async ({ page }) => {
  await page.goto("/login");
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  let playerRequests = 0;
  await page.route("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ**", async (route) => {
    playerRequests += 1;
    await route.fulfill({ status: 200, contentType: "text/html", body: "<p>Privacy player fixture</p>" });
  });
  await page.setContent(`
    <div class="youtube-loader" data-youtube-loader data-youtube-state="idle">
      <div data-youtube-consent>
        <button type="button" data-youtube-load aria-expanded="false">Load privacy player</button>
        <span>Nothing is sent to YouTube until you load this player.</span>
      </div>
      <div data-youtube-player aria-busy="false" hidden>
        <div>
          <span data-youtube-status role="status">Connecting…</span>
          <button type="button" data-youtube-unload>Unload player</button>
        </div>
        <div data-youtube-frame-wrap>
          <iframe data-youtube-frame data-src="https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ?playsinline=1&amp;rel=0&amp;start=42" title="YouTube video" referrerpolicy="strict-origin-when-cross-origin"></iframe>
        </div>
      </div>
    </div>
    <div id="action-status" role="status"></div>
  `);
  await page.addScriptTag({ url: scriptUrl });

  const frame = page.locator("[data-youtube-frame]");
  await expect(frame).not.toHaveAttribute("src");
  await expect(frame).toHaveAttribute("referrerpolicy", "strict-origin-when-cross-origin");
  expect(playerRequests).toBe(0);
  await page.getByRole("button", { name: "Load privacy player" }).click();
  await expect(frame).toHaveAttribute("src", /youtube-nocookie\.com\/embed\/dQw4w9WgXcQ/);
  await expect(page.locator("[data-youtube-frame-wrap]")).toBeVisible();
  await expect(page.locator("#action-status")).toContainText("Connecting to the privacy-enhanced YouTube player");
  await expect(page.locator("[data-youtube-status]")).toContainText("Privacy-enhanced player ready");
  await expect(page.locator("[data-youtube-player]")).toHaveAttribute("aria-busy", "false");
  await expect.poll(() => playerRequests).toBeGreaterThan(0);
  const firstRequestCount = playerRequests;

  await page.getByRole("button", { name: "Unload player" }).click();
  await expect(frame).not.toHaveAttribute("src");
  await expect(page.locator("[data-youtube-player]")).toBeHidden();
  await expect(page.getByRole("button", { name: "Load privacy player" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Load privacy player" })).toHaveAttribute("aria-expanded", "false");
  await expect(page.locator("#action-status")).toContainText("YouTube player unloaded");

  await page.getByRole("button", { name: "Load privacy player" }).click();
  await expect.poll(() => playerRequests).toBeGreaterThan(firstRequestCount);
});

test("installable app UI uses the browser prompt and reports installed state", async ({ page }) => {
  await page.goto("/login");
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  await page.setContent(`
    <fieldset data-install-app>
      <p data-install-app-status>Install RustSS from your browser.</p>
      <button type="button" data-install-app-button hidden>Install RustSS</button>
      <p data-install-app-help>Use Add to Home Screen.</p>
    </fieldset>
    <div id="action-status" role="status"></div>
  `);
  await page.addScriptTag({ url: scriptUrl });

  await page.evaluate(() => {
    window.installPromptCalls = 0;
    window.fireInstallPrompt = (outcome) => {
      const event = new Event("beforeinstallprompt", { cancelable: true });
      event.prompt = () => { window.installPromptCalls += 1; };
      event.userChoice = Promise.resolve({ outcome, platform: "web" });
      window.dispatchEvent(event);
    };
    window.fireInstallPrompt("dismissed");
  });

  const button = page.getByRole("button", { name: "Install RustSS" });
  await expect(button).toBeVisible();
  await expect(page.locator("[data-install-app-help]")).toBeHidden();
  await expect(page.locator("[data-install-app-status]")).toContainText("ready to install");
  await button.click();
  await expect.poll(() => page.evaluate(() => window.installPromptCalls)).toBe(1);
  await expect(button).toBeHidden();
  await expect(page.locator("[data-install-app-help]")).toBeVisible();
  await expect(page.locator("#action-status")).toContainText("Installation was cancelled");

  await page.evaluate(() => window.fireInstallPrompt("accepted"));
  await expect(button).toBeVisible();
  await button.click();
  await expect.poll(() => page.evaluate(() => window.installPromptCalls)).toBe(2);
  await expect(page.locator("[data-install-app-status]")).toContainText("Finishing installation");

  await page.evaluate(() => window.dispatchEvent(new Event("appinstalled")));
  await expect(button).toBeHidden();
  await expect(page.locator("[data-install-app-status]")).toContainText("is installed on this device");
  await expect(page.locator("#action-status")).toContainText("was installed on this device");
});

test("public-share controls copy the bearer link and keep risky media explicit", async ({ page }) => {
  await page.goto("/login");
  const cssUrl = new URL("/static/app.css", page.url()).href;
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  await page.setContent(`
    <link rel="stylesheet" href="${cssUrl}" />
    <details class="share-controls" open>
      <summary>Public share</summary>
      <div class="share-controls-body">
        <div class="share-link-row">
          <input value="https://reader.example/shared/secret/article" readonly data-share-link aria-label="Public article link" />
          <button type="button" data-copy-share-link>Copy link</button>
        </div>
        <form class="share-form">
          <input type="number" name="lifetime_days" value="30" data-share-lifetime aria-label="Lifetime in days" />
          <label class="check-row"><input type="checkbox" name="infinite" data-share-infinite /> No expiration</label>
          <label class="check-row"><input type="checkbox" name="include_images" /> External images (may contact publisher servers)</label>
        </form>
      </div>
    </details>
    <div id="action-status" role="status"></div>
  `);
  await page.evaluate(() => {
    window.copiedShareLink = "";
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: async (value) => { window.copiedShareLink = value; } },
    });
  });
  await page.addScriptTag({ url: scriptUrl });

  await expect(page.getByLabel("External images (may contact publisher servers)")).not.toBeChecked();
  await expect(page.getByLabel("Lifetime in days")).toBeEnabled();
  await page.getByLabel("No expiration").check();
  await expect(page.getByLabel("Lifetime in days")).toBeDisabled();
  await page.getByRole("button", { name: "Copy link" }).click();
  await expect.poll(() => page.evaluate(() => window.copiedShareLink)).toBe("https://reader.example/shared/secret/article");
  await expect(page.locator("#action-status")).toContainText("Public link copied");
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
  await page.evaluate(() => {
    window.__panelRequests = 0;
    window.fetch = function () {
      window.__panelRequests += 1;
      return Promise.resolve({
        ok: true,
        text: function () {
          return Promise.resolve('<article data-panel-article="one" data-panel-read="false" data-panel-bookmarked="false"><h2>Article opened in panel</h2></article>');
        },
      });
    };
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

test("admin queue uses the wide layout and refreshes in place", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.goto("/login");
  const cssUrl = new URL("/static/app.css", page.url()).href;
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  await page.route("**/admin/logs", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/html",
      body: '<div data-admin-queue-snapshot><strong data-test-queue-value>7</strong><details data-admin-section="active" open><summary>Active queue</summary><p>New queue rows</p></details></div>',
    });
  });
  await page.setContent(`
    <link rel="stylesheet" href="${cssUrl}" />
    <body class="page-admin page-admin-logs">
      <div class="layout no-context-sidebar">
        <main class="content">
          <div class="page-header">
            <h1>Queue &amp; logs</h1>
            <button type="button" data-admin-queue-refresh>Refresh now</button>
            <span data-admin-refresh-status></span>
          </div>
          <div data-admin-queue-snapshot><strong data-test-queue-value>1</strong><details data-admin-section="active"><summary>Active queue</summary><p>Old queue rows</p></details></div>
        </main>
      </div>
      <div id="action-status"></div>
    </body>
  `);
  await page.evaluate(() => window.history.replaceState({}, "", "/admin/logs"));
  await page.addScriptTag({ url: scriptUrl });

  expect(await page.locator("main.content").evaluate((element) => element.getBoundingClientRect().width)).toBeGreaterThan(1400);
  await page.getByRole("button", { name: "Refresh now" }).click();
  await expect(page.locator("[data-test-queue-value]")).toHaveText("7");
  await expect(page.locator("[data-admin-section='active']")).not.toHaveAttribute("open", "");
  await expect(page.locator("[data-admin-refresh-status]")).toContainText("Updated just now");
});

test("AI summary progress stays live until the priority job is ready", async ({ page }) => {
  await page.goto("/login");
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  let statusRequests = 0;
  await page.route("**/articles/one/summarize", async (route) => {
    await route.fulfill({ status: 204, body: "" });
  });
  await page.route("**/articles/one/summary/status", async (route) => {
    statusRequests += 1;
    const states = [
      { state: "queued", message: "Waiting for a priority worker…" },
      { state: "running", message: "Generating the AI summary now…" },
      { state: "ready", message: "AI summary ready." },
    ];
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(states[Math.min(statusRequests - 1, states.length - 1)]),
    });
  });
  await page.setContent(`
    <article id="card-one" data-article-card data-article-id="one">
      <form class="js-action" method="post" action="/articles/one/summarize"
            data-action="summarize" data-summary-status-url="/articles/one/summary/status"
            data-success-message="AI summary is being generated in the priority queue.">
        <input name="csrf_token" value="csrf-token">
        <button type="submit">Summarize with AI</button>
        <span data-summary-progress role="status"></span>
      </form>
    </article>
    <div id="action-status" role="status"></div>
  `);
  await page.evaluate(() => window.history.replaceState({}, "", "/articles"));
  await page.addScriptTag({ url: scriptUrl });

  const button = page.getByRole("button", { name: "Summarize with AI" });
  await expect(button).toBeEnabled();
  await button.click();
  await expect(page.locator("[data-summary-progress]")).toContainText(/priority worker|Generating/);
  await expect(page.getByRole("link", { name: "View AI summary" })).toHaveAttribute("href", "/articles/one");
  expect(statusRequests).toBeGreaterThanOrEqual(3);
});

test("dragging an article onto a sidebar folder moves its source feed", async ({ page }) => {
  await page.goto("/login");
  const scriptUrl = new URL("/static/app.js", page.url()).href;
  await page.setContent(`
    <article data-article-card><button type="button" data-feed-drag data-feed-id="feed-one" data-feed-name="Example source" draggable="true">Move source</button></article>
    <div class="folder-tree-node" data-feed-drop data-folder-id="folder-one" data-folder-label="Folder one">Folder one</div>
    <dialog data-move-feed-dialog aria-labelledby="move-feed-title">
      <form data-move-feed-form><h2 id="move-feed-title" data-move-feed-title>Choose a folder</h2><input name="csrf_token" value="csrf-token"><select name="folder_id" data-move-feed-folder><option value="folder-one">Folder one</option></select><button type="button" data-move-feed-close>Cancel</button><button type="submit">Move feed</button></form>
    </dialog>
    <div id="action-status" role="status"></div>
  `);
  await page.evaluate(() => {
    document.documentElement.setAttribute("data-csrf-token", "csrf-token");
    window.__moveRequest = null;
    window.setTimeout = function () { return 0; };
    window.fetch = function (url, options) {
      window.__moveRequest = { url: url, body: String(options.body) };
      return Promise.resolve({ ok: true });
    };
  });
  await page.addScriptTag({ url: scriptUrl });
  await page.getByRole("button", { name: "Move source" }).click();
  await expect(page.getByRole("dialog", { name: "Move Example source" })).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
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

test("side-by-side article scrolling keeps the reader panel fixed", async ({ page }) => {
  await page.goto("/login");
  const cssUrl = new URL("/static/app.css", page.url()).href;
  await page.setContent(`
    <link rel="stylesheet" href="${cssUrl}">
    <div class="layout has-context-sidebar">
      <aside class="sidebar">Sidebar</aside>
      <main class="content">
        <div class="reading-workspace">
          <section class="article-column">${"<article class='article-card'>Article</article>".repeat(80)}</section>
          <div class="panel-resizer"></div>
          <aside class="reading-pane"><div class="panel-reader">Reader panel</div></aside>
        </div>
      </main>
    </div>
  `);
  await page.evaluate(() => {
    document.documentElement.classList.add("js");
    document.body.className = "page-articles side-panel-mode";
  });
  const pane = page.locator(".reading-pane");
  const list = page.locator(".article-column");
  const initialTop = await pane.evaluate((element) => element.getBoundingClientRect().top);
  await list.evaluate((element) => { element.scrollTop = 900; });
  await expect.poll(() => list.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  const finalTop = await pane.evaluate((element) => element.getBoundingClientRect().top);
  expect(finalTop).toBe(initialTop);
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
});

test("reader typography preserves rich article structure", async ({ page }) => {
  await page.goto("/login");
  const cssUrl = new URL("/static/app.css", page.url()).href;
  await page.setContent(`
    <link rel="stylesheet" href="${cssUrl}">
    <article class="panel-reader">
      <header><h2>Reader title</h2></header>
      <div class="article-body">
        <h2>Article section</h2>
        <p data-empty-paragraph></p>
        <p>Text with <code>inline()</code>.</p>
        <ul><li>First<ul><li>Nested</li></ul></li><li>Second</li></ul>
        <pre><code>const longLine = "${"x".repeat(180)}";</code></pre>
        <table><thead><tr><th>Name</th><th>Value</th></tr></thead><tbody><tr><td>Alpha</td><td>Beta</td></tr></tbody></table>
        <blockquote><p>Quoted text</p></blockquote>
      </div>
    </article>
  `);

  const articleHeading = page.locator(".article-body h2");
  const title = page.locator(".panel-reader > header h2");
  expect(parseFloat(await articleHeading.evaluate((element) => getComputedStyle(element).fontSize)))
    .toBeGreaterThan(parseFloat(await title.evaluate((element) => getComputedStyle(element).fontSize)) * 0.9);
  await expect(page.locator(".article-body > ul")).toHaveCSS("list-style-type", "disc");
  await expect(page.locator(".article-body ul ul")).toHaveCSS("list-style-type", "circle");
  await expect(page.locator(".article-body pre")).toHaveCSS("overflow-x", "auto");
  await expect(page.locator(".article-body pre")).toHaveCSS("white-space", "pre");
  await expect(page.locator(".article-body table")).toHaveCSS("overflow-x", "auto");
  await expect(page.locator(".article-body blockquote")).toHaveCSS("border-left-style", "solid");
  await expect(page.locator("[data-empty-paragraph]")).toBeHidden();
});

test("locale direction and personal CSS safe mode persist across navigation", async ({ page }, testInfo) => {
  await register(page, `locale-customization-${Date.now()}-${testInfo.parallelIndex}`);

  await page.goto("/settings/preferences");
  await page.getByLabel("Interface language").selectOption("ar");
  await page.getByRole("button", { name: "Save preferences" }).click();
  await expect(page.locator("html")).toHaveAttribute("lang", "ar");
  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  await expect(page.getByRole("link", { name: "المكتبة", exact: true })).toBeVisible();
  await page.goto("/articles");
  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");

  await page.goto("/settings/customization");
  const css = ".article-card { border-color: rgb(12, 34, 56); }";
  await page.locator("[data-css-customization]").fill(css);
  await page.getByRole("button", { name: "Save new CSS version" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Personal CSS version saved" })).toBeVisible();
  await expect(page.getByText(/Version 1 .* active/)).toBeVisible();

  const personalized = await page.request.get("/assets/personal.css");
  expect(personalized.status()).toBe(200);
  expect(await personalized.text()).toContain("rgb(12, 34, 56)");

  await page.goto("/safe-mode");
  await expect(page.getByRole("alert")).toContainText("Safe mode is active");
  const suppressed = await page.request.get("/assets/personal.css");
  expect(suppressed.status()).toBe(200);
  expect(await suppressed.text()).toBe("");

  await page.getByRole("link", { name: "Leave safe mode" }).click();
  await expect(page.getByText("Safe mode is active")).toHaveCount(0);
  const restored = await page.request.get("/assets/personal.css");
  expect(await restored.text()).toContain("rgb(12, 34, 56)");
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
