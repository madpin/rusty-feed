// RustSS progressive enhancement (M5). The UI works fully without JS; this just
// makes actions in-place and enables inline reading. Vanilla JS, no dependencies.
(function () {
  "use strict";

  var status = document.getElementById("action-status");
  var statusTimer;

  function announce(message, isError) {
    if (!status) return;
    status.textContent = message;
    status.classList.toggle("error", Boolean(isError));
    status.classList.add("visible");
    window.clearTimeout(statusTimer);
    statusTimer = window.setTimeout(function () {
      status.classList.remove("visible");
    }, 4200);
  }

  function encodedForm(form) {
    var body = new URLSearchParams();
    new FormData(form).forEach(function (value, key) {
      if (typeof value === "string") body.append(key, value);
    });
    return body;
  }

  function setPending(form, pending) {
    form.setAttribute("aria-busy", pending ? "true" : "false");
    form.dataset.pending = pending ? "true" : "false";
    Array.prototype.forEach.call(form.querySelectorAll("button, input[type='submit']"), function (control) {
      control.disabled = pending;
    });
  }

  function targetFor(form) {
    var selector = form.getAttribute("data-target");
    return selector ? document.querySelector(selector) : null;
  }

  function swapToggle(form) {
    var altAction = form.getAttribute("data-alt-action");
    if (altAction) {
      form.setAttribute("data-alt-action", form.action);
      form.action = altAction;
    }
    var button = form.querySelector("button");
    var altLabel = form.getAttribute("data-alt-label");
    if (button && altLabel) {
      form.setAttribute("data-alt-label", button.textContent.trim());
      button.textContent = altLabel;
    }
  }

  function updateFeedback(target, value) {
    if (!target) return;
    target.classList.toggle("rated", value === "up" || value === "down");
    target.classList.toggle("rated-positive", value === "up");
    target.classList.toggle("rated-negative", value === "down");
    Array.prototype.forEach.call(target.querySelectorAll("[data-feedback-button]"), function (button) {
      button.setAttribute("aria-pressed", button.getAttribute("data-feedback-button") === value ? "true" : "false");
    });
  }

  function setToggleState(form, action, alternative, label, alternativeLabel) {
    if (!form) return;
    form.action = action;
    form.setAttribute("data-alt-action", alternative);
    form.setAttribute("data-alt-label", alternativeLabel);
    var button = form.querySelector("button");
    if (button) button.textContent = label;
  }

  function syncCardState(card, panelArticle) {
    if (!card || !panelArticle) return;
    var id = panelArticle.getAttribute("data-panel-article");
    var read = panelArticle.getAttribute("data-panel-read") === "true";
    var bookmarked = panelArticle.getAttribute("data-panel-bookmarked") === "true";
    var feedback = panelArticle.getAttribute("data-panel-feedback");
    card.classList.toggle("read", read);
    card.classList.toggle("bookmarked", bookmarked);
    setToggleState(
      card.querySelector("[data-action='read-toggle']"),
      "/articles/" + id + (read ? "/unread" : "/read"),
      "/articles/" + id + (read ? "/read" : "/unread"),
      read ? "Mark unread" : "Mark read",
      read ? "Mark read" : "Mark unread"
    );
    setToggleState(
      card.querySelector("[data-action='bookmark-toggle']"),
      "/articles/" + id + (bookmarked ? "/unbookmark" : "/bookmark"),
      "/articles/" + id + (bookmarked ? "/bookmark" : "/unbookmark"),
      bookmarked ? "Saved" : "Save",
      bookmarked ? "Save" : "Saved"
    );
    updateFeedback(card, feedback);
  }

  function reportReadingWithin(root) {
    Array.prototype.forEach.call(root.querySelectorAll("[data-track-reading]"), function (node) {
      if (typeof node._rustssReportReading === "function") node._rustssReportReading();
    });
  }

  function initializeReadingTracking(root) {
    var tracked = [];
    if (root.matches && root.matches("[data-track-reading]")) tracked.push(root);
    Array.prototype.push.apply(tracked, root.querySelectorAll("[data-track-reading]"));
    tracked.forEach(function (node) {
      if (node.dataset.readingTracker === "true") return;
      node.dataset.readingTracker = "true";
      var startedAt = Date.now();
      var sent = false;
      var report = function () {
        if (sent) return;
        sent = true;
        var seconds = Math.max(1, Math.round((Date.now() - startedAt) / 1000));
        var body = new URLSearchParams();
        body.set("csrf_token", node.getAttribute("data-csrf") || "");
        body.set("time_spent", String(seconds));
        var url = node.getAttribute("data-track-reading");
        var delivered = false;
        if (navigator.sendBeacon) {
          delivered = navigator.sendBeacon(
            url,
            new Blob([body.toString()], { type: "application/x-www-form-urlencoded;charset=UTF-8" })
          );
        }
        if (!delivered) {
          fetch(url, { method: "POST", body: body, keepalive: true, credentials: "same-origin" }).catch(function () {});
        }
      };
      node._rustssReportReading = report;
      document.addEventListener("visibilitychange", function () {
        if (document.visibilityState === "hidden") report();
      });
      window.addEventListener("pagehide", report);
    });
  }

  function sidePanelAvailable(workspace) {
    var pane = workspace ? workspace.querySelector(".reading-pane") : null;
    return Boolean(pane && window.getComputedStyle(pane).display !== "none");
  }

  function persistPanelWidth(width) {
    var root = document.documentElement;
    var url = root.getAttribute("data-layout-url");
    var csrf = root.getAttribute("data-csrf-token");
    if (!url || !csrf) return;
    var body = new URLSearchParams();
    body.set("csrf_token", csrf);
    body.set("reader_panel_width", String(width));
    fetch(url, {
      method: "POST",
      body: body,
      headers: { "X-Requested-With": "fetch" },
      credentials: "same-origin",
    })
      .then(function (response) {
        if (!response.ok) throw new Error("layout save failed");
        announce("Reading panel width saved.", false);
      })
      .catch(function () {
        announce("Could not save the reading panel width.", true);
      });
  }

  function syncSidebarToggle(buttons, collapsed) {
    Array.prototype.forEach.call(buttons, function (button) {
      button.setAttribute("aria-expanded", collapsed ? "false" : "true");
      button.setAttribute("aria-label", collapsed ? "Expand sidebar" : "Reduce sidebar");
      var glyph = button.querySelector("[data-sidebar-toggle-glyph]");
      if (glyph) glyph.textContent = collapsed ? "›" : "‹";
      var label = button.querySelector("[data-sidebar-toggle-label]");
      if (label) label.textContent = collapsed ? "Expand sidebar" : "Reduce sidebar";
    });
  }

  function filterSelectOptions(select, query) {
    if (!select) return;
    var text = (query || "").trim().toLowerCase();
    Array.prototype.forEach.call(select.options, function (option) {
      option.hidden = Boolean(text && option.value && option.textContent.toLowerCase().indexOf(text) === -1);
    });
  }

  function initializeFeedOrganizer() {
    var dialog = document.querySelector("[data-feed-organizer]");
    var form = dialog ? dialog.querySelector("[data-feed-organizer-form]") : null;
    if (!dialog || !form || typeof dialog.showModal !== "function") return;
    document.documentElement.classList.add("feed-organizer-ready");
    var title = dialog.querySelector("[data-feed-organizer-title]");
    var folderSelect = dialog.querySelector("[data-folder-select]");
    var folderSearch = dialog.querySelector("[data-folder-search]");
    var tagsInput = dialog.querySelector("[data-feed-organizer-tags]");
    var tagSelect = dialog.querySelector("[data-tag-select]");
    var tagSearch = dialog.querySelector("[data-tag-search]");
    function close() {
      if (dialog.open) dialog.close();
    }
    function selectTags(tags) {
      if (!tagSelect) return tags;
      var known = {};
      Array.prototype.forEach.call(tagSelect.options, function (option) {
        known[option.value] = true;
        option.selected = tags.indexOf(option.value) !== -1;
      });
      return tags.filter(function (tag) { return !known[tag]; });
    }
    Array.prototype.forEach.call(document.querySelectorAll("[data-feed-organizer-trigger]"), function (trigger) {
      trigger.addEventListener("click", function () {
        var tags = (trigger.getAttribute("data-feed-tags") || "")
          .split(",")
          .map(function (tag) { return tag.trim(); })
          .filter(Boolean);
        form.action = "/feeds/" + encodeURIComponent(trigger.getAttribute("data-feed-id")) + "/organization";
        if (title) title.textContent = "Organize " + (trigger.getAttribute("data-feed-name") || "feed");
        if (folderSelect) folderSelect.value = trigger.getAttribute("data-feed-folder-id") || "";
        if (folderSearch) {
          folderSearch.value = "";
          filterSelectOptions(folderSelect, "");
        }
        var customTags = selectTags(tags);
        if (tagsInput) tagsInput.value = customTags.join(", ");
        if (tagSearch) {
          tagSearch.value = "";
          filterSelectOptions(tagSelect, "");
        }
        dialog.showModal();
      });
    });
    Array.prototype.forEach.call(dialog.querySelectorAll("[data-feed-organizer-close]"), function (button) {
      button.addEventListener("click", close);
    });
    if (folderSearch) folderSearch.addEventListener("input", function () { filterSelectOptions(folderSelect, folderSearch.value); });
    if (tagSearch) tagSearch.addEventListener("input", function () { filterSelectOptions(tagSelect, tagSearch.value); });
    dialog.addEventListener("click", function (event) {
      if (event.target === dialog) close();
    });
  }

  function initializeSidebarToggle() {
    var root = document.documentElement;
    var buttons = document.querySelectorAll("[data-sidebar-toggle]");
    if (!buttons.length) return;
    syncSidebarToggle(buttons, root.getAttribute("data-sidebar-collapsed") === "true");
    Array.prototype.forEach.call(buttons, function (button) {
      button.addEventListener("click", function () {
        var wasCollapsed = root.getAttribute("data-sidebar-collapsed") === "true";
        var collapsed = !wasCollapsed;
        var url = root.getAttribute("data-layout-url");
        var csrf = root.getAttribute("data-csrf-token");
        root.setAttribute("data-sidebar-collapsed", String(collapsed));
        syncSidebarToggle(buttons, collapsed);
        if (!url || !csrf) return;
        var body = new URLSearchParams();
        body.set("csrf_token", csrf);
        body.set("sidebar_collapsed", String(collapsed));
        fetch(url, {
          method: "POST",
          body: body,
          headers: { "X-Requested-With": "fetch" },
          credentials: "same-origin",
        })
          .then(function (response) {
            if (!response.ok) throw new Error("sidebar state save failed");
            announce(collapsed ? "Sidebar reduced." : "Sidebar expanded.", false);
          })
          .catch(function () {
            root.setAttribute("data-sidebar-collapsed", String(wasCollapsed));
            syncSidebarToggle(buttons, wasCollapsed);
            announce("Could not save the sidebar state.", true);
          });
      });
    });
  }

  function initializePanelResize() {
    var workspace = document.querySelector(".reading-workspace");
    var handle = workspace ? workspace.querySelector(".panel-resizer") : null;
    if (!workspace || !handle) return;
    var saveTimer;
    var startWidth = Number(handle.getAttribute("aria-valuenow")) || 56;
    function bounds() {
      var width = workspace.clientWidth;
      var min = Math.max(38, Math.ceil((20 * 16 * 100) / width));
      var max = Math.min(68, Math.floor(((width - 16 * 16 - 24) * 100) / width));
      return { min: min, max: Math.max(min, max) };
    }
    function setWidth(value) {
      var range = bounds();
      var width = Math.round(Math.max(range.min, Math.min(range.max, value)));
      workspace.style.setProperty("--reader-panel-width", width + "%");
      handle.setAttribute("aria-valuemin", String(range.min));
      handle.setAttribute("aria-valuemax", String(range.max));
      handle.setAttribute("aria-valuenow", String(width));
      handle.setAttribute("aria-valuetext", "Reading panel " + width + " percent wide");
      return width;
    }
    function scheduleSave(width) {
      window.clearTimeout(saveTimer);
      saveTimer = window.setTimeout(function () { persistPanelWidth(width); }, 300);
    }
    function stopResizing(event, save) {
      if (handle.dataset.resizing !== "true") return;
      handle.dataset.resizing = "false";
      if (event && handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
      if (save) scheduleSave(Number(handle.getAttribute("aria-valuenow")) || 56);
    }
    function widthFromPointer(clientX) {
      var rect = workspace.getBoundingClientRect();
      return ((rect.right - clientX) / rect.width) * 100;
    }
    setWidth(startWidth);
    handle.addEventListener("pointerdown", function (event) {
      if (!sidePanelAvailable(workspace) || event.button !== 0) return;
      startWidth = Number(handle.getAttribute("aria-valuenow")) || startWidth;
      handle.dataset.resizing = "true";
      handle.setPointerCapture(event.pointerId);
      setWidth(widthFromPointer(event.clientX));
      event.preventDefault();
    });
    handle.addEventListener("pointermove", function (event) {
      if (handle.dataset.resizing === "true") setWidth(widthFromPointer(event.clientX));
    });
    handle.addEventListener("pointerup", function (event) { stopResizing(event, true); });
    handle.addEventListener("pointercancel", function (event) { stopResizing(event, false); });
    handle.addEventListener("lostpointercapture", function () { handle.dataset.resizing = "false"; });
    handle.addEventListener("keydown", function (event) {
      var current = Number(handle.getAttribute("aria-valuenow")) || 56;
      var next;
      if (event.key === "ArrowLeft") next = current + 2;
      else if (event.key === "ArrowRight") next = current - 2;
      else if (event.key === "Home") next = bounds().min;
      else if (event.key === "End") next = bounds().max;
      else if (event.key === "Escape" && handle.dataset.resizing === "true") {
        setWidth(startWidth);
        stopResizing(null, false);
        event.preventDefault();
        return;
      } else return;
      scheduleSave(setWidth(next));
      event.preventDefault();
    });
  }

  function loadPanel(panel, url, trigger) {
    reportReadingWithin(panel);
    panel.setAttribute("aria-busy", "true");
    panel.innerHTML = '<div class="reading-pane-empty">Loading article…</div>';
    return fetch(url, { headers: { "X-Requested-With": "fetch" }, credentials: "same-origin" })
      .then(function (response) {
        if (!response.ok) throw new Error("panel failed");
        return response.text();
      })
      .then(function (html) {
        panel.innerHTML = html;
        panel.removeAttribute("aria-busy");
        initializeReadingTracking(panel);
        if (trigger) {
          var list = trigger.closest(".article-list");
          if (list) {
            Array.prototype.forEach.call(list.querySelectorAll(".article-card.selected"), function (card) {
              card.classList.remove("selected");
            });
            Array.prototype.forEach.call(list.querySelectorAll(".article-link[aria-expanded='true']"), function (link) {
              link.setAttribute("aria-expanded", "false");
            });
          }
          var card = trigger.closest(".article-card");
          if (card) {
            card.classList.add("selected");
            syncCardState(card, panel.querySelector("[data-panel-article]"));
          }
          trigger.setAttribute("aria-expanded", "true");
        }
        var heading = panel.querySelector("h2");
        if (heading) {
          heading.setAttribute("tabindex", "-1");
          heading.focus({ preventScroll: true });
        }
      })
      .catch(function (error) {
        panel.removeAttribute("aria-busy");
        throw error;
      });
  }

  function toggleLiveView(button) {
    var article = button.closest("article");
    var reader = article ? article.querySelector("[data-reader-view-content]") : null;
    var live = article ? article.querySelector("[data-live-view]") : null;
    var frame = live ? live.querySelector("[data-live-view-frame]") : null;
    if (!reader || !live || !frame) return;
    var showLive = button.getAttribute("aria-expanded") !== "true";
    if (showLive) {
      var source = frame.getAttribute("data-live-src");
      if (!source) return;
      live.hidden = false;
      reader.hidden = true;
      live.setAttribute("aria-busy", "true");
      frame.addEventListener("load", function () {
        if (!live.hidden) live.removeAttribute("aria-busy");
      }, { once: true });
      frame.addEventListener("error", function () {
        if (!live.hidden) {
          live.removeAttribute("aria-busy");
          announce("Live view could not be loaded. Open the original article in a new tab.", true);
        }
      }, { once: true });
      frame.setAttribute("src", source);
    } else {
      frame.removeAttribute("src");
      live.hidden = true;
      live.removeAttribute("aria-busy");
      reader.hidden = false;
    }
    button.setAttribute("aria-expanded", String(showLive));
    button.textContent = showLive ? "Reader view" : "Live view";
    announce(showLive ? "Live view opened." : "Reader view restored.", false);
  }

  document.addEventListener("click", function (event) {
    var button = event.target.closest ? event.target.closest("[data-live-view-toggle]") : null;
    if (!button) return;
    event.preventDefault();
    toggleLiveView(button);
  });

  // In-place article actions (mark read/unread, save/unsave, mark all read).
  document.addEventListener("submit", function (event) {
    var form = event.target;
    if (!form.classList || !form.classList.contains("js-action")) return;
    event.preventDefault();
    if (form.dataset.pending === "true") return;

    var actionBefore = form.action;
    var markingRead = /\/read$/.test(new URL(actionBefore, window.location.href).pathname);
    setPending(form, true);
    fetch(form.action, {
      method: "POST",
      body: encodedForm(form),
      headers: { "X-Requested-With": "fetch" },
      credentials: "same-origin",
    })
      .then(function (response) {
        if (!response.ok) throw new Error("action failed");
        return response;
      })
      .then(function () {
        if (form.getAttribute("data-action") === "mark-all-read") {
          announce(form.getAttribute("data-success-message") || "All articles marked read.", false);
          window.setTimeout(function () { window.location.reload(); }, 100);
          return null;
        }

        var target = targetFor(form);
        var toggle = form.getAttribute("data-toggle-class");
        if (target && toggle) target.classList.toggle(toggle);
        if (target && markingRead) {
          var next = form.querySelector("input[name='next']");
          if (next && /(?:^|[?&])unread=1(?:&|$)/.test(next.value)) target.remove();
        }
        updateFeedback(target, form.getAttribute("data-feedback-value"));
        swapToggle(form);

        if (form.getAttribute("data-refresh-panel") === "true") {
          var panel = form.closest(".article-inline") || form.closest(".reading-pane");
          var panelUrl = form.getAttribute("data-panel-url");
          if (panel && panelUrl) {
            return loadPanel(panel, panelUrl, null).then(function () {
              var panelArticle = panel.querySelector("[data-panel-article]");
              var card = panelArticle ? document.getElementById("card-" + panelArticle.getAttribute("data-panel-article")) : null;
              syncCardState(card, panelArticle);
            });
          }
        }
        return null;
      })
      .then(function () {
        if (form.getAttribute("data-action") !== "mark-all-read") {
          announce(form.getAttribute("data-success-message") || "Saved.", false);
        }
      })
      .catch(function () {
        announce("That action could not be completed. Please try again.", true);
      })
      .finally(function () {
        setPending(form, false);
      });
  });

  document.addEventListener("change", function (event) {
    var form = event.target.closest ? event.target.closest("[data-catalog-filters]") : null;
    if (!form || event.target.disabled) return;
    form.requestSubmit();
  });

  // Implicit reading signal: on an article page, report time spent when the
  // reader leaves, so personalization can learn from completion/bounce.
  initializeReadingTracking(document);
  initializeFeedOrganizer();
  initializeSidebarToggle();
  initializePanelResize();

  // Inline reading: expand article content in the list instead of navigating.
  document.addEventListener("click", function (event) {
    var link = event.target.closest ? event.target.closest("a.article-link") : null;
    if (!link) return;
    var list = link.closest(".article-list");
    if (!list) return;
    var mode = list.getAttribute("data-reading-mode");
    if (mode !== "inline" && mode !== "side-panel") return;
    var workspace = list.closest(".reading-workspace");
    if (mode === "side-panel" && !sidePanelAvailable(workspace)) return;

    event.preventDefault();
    var panel;
    if (mode === "inline") {
      panel = link.closest(".article-card").querySelector(".article-inline");
      if (!panel.hidden && panel.getAttribute("data-panel-url") === link.href + "/panel") {
        reportReadingWithin(panel);
        panel.hidden = true;
        link.setAttribute("aria-expanded", "false");
        link.closest(".article-card").classList.remove("selected");
        return;
      }
      Array.prototype.forEach.call(list.querySelectorAll(".article-inline"), function (other) {
        if (other !== panel) {
          reportReadingWithin(other);
          other.hidden = true;
          other.closest(".article-card").classList.remove("selected");
        }
      });
      panel.hidden = false;
    } else {
      var panelSelector = list.getAttribute("data-reading-panel");
      panel = panelSelector ? document.querySelector(panelSelector) : null;
    }
    if (!panel) {
      window.location.assign(link.href);
      return;
    }
    panel.setAttribute("data-panel-url", link.href + "/panel");
    loadPanel(panel, link.href + "/panel", link)
      .then(function () { announce("Article opened.", false); })
      .catch(function () { window.location.assign(link.href); });
  });

  document.addEventListener("click", function (event) {
    var button = event.target.closest ? event.target.closest("[data-copy-token]") : null;
    if (!button) return;
    var value = document.getElementById(button.getAttribute("data-copy-token"));
    if (!value) return;
    var text = value.textContent || "";
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(text)
        .then(function () { announce("Token copied.", false); })
        .catch(function () { announce("Could not copy the token. Select it manually.", true); });
      return;
    }
    var range = document.createRange();
    range.selectNodeContents(value);
    var selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    try {
      document.execCommand("copy");
      announce("Token copied.", false);
    } catch (_) {
      announce("Could not copy the token. Select it manually.", true);
    }
    selection.removeAllRanges();
  });

  function shortcutFromEvent(event) {
    if (event.ctrlKey || event.altKey || event.metaKey || event.getModifierState("AltGraph")) return null;
    var key = event.key;
    if (key.length === 1) key = key.toLowerCase();
    else if (["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "PageDown", "PageUp", "Home", "End"].indexOf(key) === -1) return null;
    if (["Control", "Alt", "Shift", "Meta"].indexOf(key) !== -1) return null;
    return event.shiftKey ? "Shift+" + key : key;
  }

  function shortcutInputs() {
    return Array.prototype.slice.call(document.querySelectorAll("[data-shortcut-input]"));
  }

  function validateShortcutInputs() {
    var seen = {};
    shortcutInputs().forEach(function (input) {
      input.setCustomValidity("");
      var value = input.value.trim();
      if (!value) return;
      if (seen[value]) {
        input.setCustomValidity("Each shortcut must be unique.");
        seen[value].setCustomValidity("Each shortcut must be unique.");
      } else {
        seen[value] = input;
      }
    });
  }

  function currentArticleCard() {
    var focused = document.activeElement && document.activeElement.closest ? document.activeElement.closest(".article-card") : null;
    return focused || document.querySelector(".article-card.selected");
  }

  function openAdjacentArticle(direction) {
    var cards = Array.prototype.slice.call(document.querySelectorAll(".article-card"));
    if (!cards.length) return false;
    var focused = document.activeElement && document.activeElement.closest ? document.activeElement.closest(".article-card") : null;
    var current = focused || document.querySelector(".article-card.selected");
    var index = cards.indexOf(current);
    if (index >= 0 && (index + direction < 0 || index + direction >= cards.length)) {
      announce(direction > 0 ? "Last article in this list." : "First article in this list.", false);
      return true;
    }
    var target = cards[index < 0 ? (direction > 0 ? 0 : cards.length - 1) : index + direction];
    var link = target.querySelector(".article-link");
    if (!link) return false;
    link.focus();
    link.click();
    return true;
  }

  function submitCurrentAction(name) {
    var scope = currentArticleCard() || document.querySelector("[data-panel-article], [data-current-article]");
    if (!scope) return false;
    var form = scope.querySelector("[data-action='" + name + "']");
    if (!form) return false;
    var path = new URL(form.action, window.location.href).pathname;
    if (name === "read-toggle" && /\/read$/.test(path)) {
      announce("This article is already unread.", false);
      return true;
    }
    if (name === "bookmark-toggle" && /\/unbookmark$/.test(path)) {
      announce("This article is already saved.", false);
      return true;
    }
    var button = form.querySelector("button");
    if (!button || button.disabled) return false;
    button.click();
    return true;
  }

  function changeFeed(direction) {
    var seen = {};
    var links = Array.prototype.slice.call(document.querySelectorAll("[data-feed-nav]")).filter(function (link) {
      if (!link.getClientRects().length || seen[link.href]) return false;
      seen[link.href] = true;
      return true;
    });
    if (!links.length) return false;
    var active = links.findIndex(function (link) { return link.classList.contains("active"); });
    if (active >= 0 && (active + direction < 0 || active + direction >= links.length)) {
      announce(direction > 0 ? "Last feed in this list." : "First feed in this list.", false);
      return true;
    }
    var next = active < 0 ? (direction > 0 ? 0 : links.length - 1) : active + direction;
    window.location.assign(links[next].href);
    return true;
  }

  document.addEventListener("keydown", function (event) {
    var input = event.target.closest ? event.target.closest("[data-shortcut-input]") : null;
    if (input) {
      if (event.key === "Escape") {
        input.blur();
        announce("Shortcut recording cancelled.", false);
        event.preventDefault();
        return;
      }
      if (event.key === "Backspace" || event.key === "Delete") {
        input.value = "";
        validateShortcutInputs();
        event.preventDefault();
        return;
      }
      if (event.key === "Tab") return;
      var captured = shortcutFromEvent(event);
      if (captured) {
        input.value = captured;
        validateShortcutInputs();
        announce(captured + " recorded. Save preferences to keep it.", false);
      } else {
        announce("Use one printable key, an arrow, Page Up/Down, Home, or End; Ctrl, Alt, and Command are reserved.", true);
      }
      event.preventDefault();
      return;
    }
    if (event.defaultPrevented || event.repeat || event.isComposing || event.keyCode === 229) return;
    if (document.querySelector(".account-menu[open], dialog[open]")) return;
    if (event.target.closest && event.target.closest("input, textarea, select, button, summary, iframe, [contenteditable], [role='textbox'], [role='combobox'], audio, video")) return;
    var root = document.documentElement;
    if (root.getAttribute("data-shortcuts-enabled") !== "true") return;
    var shortcut = shortcutFromEvent(event);
    if (!shortcut) return;
    var handled = false;
    if (shortcut === root.getAttribute("data-shortcut-next-article")) handled = openAdjacentArticle(1);
    else if (shortcut === root.getAttribute("data-shortcut-previous-article")) handled = openAdjacentArticle(-1);
    else if (shortcut === root.getAttribute("data-shortcut-save-article")) handled = submitCurrentAction("bookmark-toggle");
    else if (shortcut === root.getAttribute("data-shortcut-mark-unread")) handled = submitCurrentAction("read-toggle");
    else if (shortcut === root.getAttribute("data-shortcut-next-feed")) handled = changeFeed(1);
    else if (shortcut === root.getAttribute("data-shortcut-previous-feed")) handled = changeFeed(-1);
    if (handled) event.preventDefault();
  });

  document.addEventListener("click", function (event) {
    var button = event.target.closest ? event.target.closest("[data-reset-shortcuts]") : null;
    if (!button) return;
    var defaults = {
      shortcut_next_article: "ArrowDown",
      shortcut_previous_article: "ArrowUp",
      shortcut_save_article: "s",
      shortcut_mark_unread: "u",
      shortcut_next_feed: "ArrowRight",
      shortcut_previous_feed: "ArrowLeft",
    };
    shortcutInputs().forEach(function (input) {
      input.value = defaults[input.name] || "";
    });
    var enabled = document.querySelector("input[name='shortcuts_enabled']");
    if (enabled) enabled.checked = true;
    validateShortcutInputs();
    announce("Shortcut defaults restored. Save preferences to keep them.", false);
  });
})();
