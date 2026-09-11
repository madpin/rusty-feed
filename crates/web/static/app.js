// @ts-check
// Rusty Feed progressive enhancement (M5). The UI works fully without JS; this just
// makes actions in-place and enables inline reading. Vanilla JS, no dependencies.
(function () {
  "use strict";

  document.documentElement.classList.add("js");

  var status = document.getElementById("action-status");
  var statusTimer;
  var installPrompt = null;

  document.addEventListener("submit", function (event) {
    var form = event.target;
    if (!form || !form.getAttribute) return;
    var message = form.getAttribute("data-confirm");
    if (!message && event.submitter && event.submitter.getAttribute) {
      message = event.submitter.getAttribute("data-confirm");
    }
    if (message && !window.confirm(message)) event.preventDefault();
  });

  document.addEventListener("click", function (event) {
    var field = event.target.closest ? event.target.closest("[data-select-on-click]") : null;
    if (field && typeof field.select === "function") field.select();
  });

  function installedDisplayMode() {
    return window.matchMedia && window.matchMedia("(display-mode: standalone)").matches;
  }

  function updateInstallApp(mode) {
    var panel = document.querySelector("[data-install-app]");
    if (!panel) return;
    var button = panel.querySelector("[data-install-app-button]");
    var help = panel.querySelector("[data-install-app-help]");
    var installStatus = panel.querySelector("[data-install-app-status]");
    var appName = panel.getAttribute("data-install-app-name") || "Rusty Feed";
    if (mode === "installed") {
      if (button) button.hidden = true;
      if (help) help.hidden = true;
      if (installStatus) installStatus.textContent = appName + " is installed on this device and opens in its own window.";
      return;
    }
    if (mode === "prompt") {
      if (button) {
        button.hidden = false;
        button.disabled = false;
      }
      if (help) help.hidden = true;
      if (installStatus) installStatus.textContent = "This browser is ready to install Rusty Feed as an app.";
      return;
    }
    if (button) button.hidden = true;
    if (help) help.hidden = false;
  }

  window.addEventListener("beforeinstallprompt", function (event) {
    event.preventDefault();
    installPrompt = event;
    updateInstallApp("prompt");
  });

  window.addEventListener("appinstalled", function () {
    installPrompt = null;
    updateInstallApp("installed");
    var panel = document.querySelector("[data-install-app]");
    var appName = panel ? panel.getAttribute("data-install-app-name") || "Rusty Feed" : "Rusty Feed";
    announce(appName + " was installed on this device.", false);
  });

  document.addEventListener("click", function (event) {
    var button = event.target.closest ? event.target.closest("[data-install-app-button]") : null;
    if (!button || !installPrompt) return;
    var prompt = installPrompt;
    installPrompt = null;
    button.disabled = true;
    prompt.prompt();
    prompt.userChoice.then(function (choice) {
      if (choice.outcome === "accepted") {
        var installStatus = document.querySelector("[data-install-app-status]");
        if (installStatus) installStatus.textContent = "Finishing installation…";
      } else {
        updateInstallApp("manual");
        announce("Installation was cancelled. You can use the browser menu whenever you are ready.", false);
      }
    }).catch(function () {
      updateInstallApp("manual");
    });
  });

  if (installedDisplayMode()) updateInstallApp("installed");

  function registerServiceWorker() {
    if (!("serviceWorker" in navigator) || !window.isSecureContext) return;
    navigator.serviceWorker.register("/service-worker.js", { scope: "/" }).catch(function () {});
  }
  if (document.readyState === "complete") registerServiceWorker();
  else window.addEventListener("load", registerServiceWorker, { once: true });

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
      if (button.hasAttribute("data-icon-button")) {
        // Icon buttons keep their glyph; the label lives on aria-label (which
        // also drives the hover/focus tooltip), so swap that instead of wiping
        // the button's contents.
        form.setAttribute("data-alt-label", (button.getAttribute("aria-label") || "").trim());
        button.setAttribute("aria-label", altLabel);
      } else {
        form.setAttribute("data-alt-label", button.textContent.trim());
        button.textContent = altLabel;
      }
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
    if (button) {
      if (button.hasAttribute("data-icon-button")) button.setAttribute("aria-label", label);
      else button.textContent = label;
    }
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
      if (typeof node._rustyFeedReportReading === "function") node._rustyFeedReportReading();
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
      node._rustyFeedReportReading = report;
      document.addEventListener("visibilitychange", function () {
        if (document.visibilityState === "hidden") report();
      });
      window.addEventListener("pagehide", report);
    });
  }

  function initializePendingContent(root) {
    Array.prototype.forEach.call(root.querySelectorAll("[data-content-pending='true']"), function (node) {
      if (node.dataset.contentPoll === "true") return;
      node.dataset.contentPoll = "true";
      var attempts = 0;
      var deadline = Date.now() + (5 * 60 * 1000);
      function markDelayed() {
        node.removeAttribute("data-content-pending");
        var article = node.closest("article");
        var notice = article ? article.querySelector(".content-notice") : null;
        if (notice) notice.textContent = "The public-page update is taking longer than usual. Stored RSS content remains available while the background queue continues.";
      }
      var poll = function () {
        var url = node.getAttribute("data-content-url");
        if (!url || !node.isConnected) return;
        fetch(url, { credentials: "same-origin", headers: { "X-Requested-With": "fetch" } })
          .then(function (response) {
            if (!response.ok) throw new Error("content refresh failed");
            var contentStatus = response.headers.get("x-rusty-feed-content-status") || "ready";
            return response.text().then(function (html) { return { html: html, status: contentStatus }; });
          })
          .then(function (result) {
            if (result.status === "ready") {
              node.innerHTML = result.html;
              node.removeAttribute("data-content-pending");
              var article = node.closest("article");
              var notice = article ? article.querySelector(".content-notice") : null;
              if (notice) notice.remove();
              announce("Public-page content is ready.", false);
              return;
            }
            if (result.status === "failed") {
              node.removeAttribute("data-content-pending");
              var failedArticle = node.closest("article");
              var failedNotice = failedArticle ? failedArticle.querySelector(".content-notice") : null;
              if (failedNotice) failedNotice.textContent = "The public page could not be updated. Stored RSS content remains available; Rusty Feed will retry later.";
              announce("The public page could not be updated. Stored RSS content remains available.", true);
              return;
            }
            attempts += 1;
            if (Date.now() < deadline) {
              window.setTimeout(poll, Math.min(750 * attempts, 5000));
            } else {
              markDelayed();
            }
          })
          .catch(function () {
            attempts += 1;
            if (Date.now() < deadline) {
              window.setTimeout(poll, Math.min(1000 * attempts, 5000));
            } else {
              markDelayed();
            }
          });
      };
      window.setTimeout(poll, 500);
    });
  }

  // Whether finishing this job will show the actual summary right on this
  // view (a panel refresh, or the standalone article reloading into "At a
  // glance") rather than just swapping in a "View AI summary" link (the list
  // card case, where the content itself stays off-screen). Used to skip a
  // redundant "ready" toast when the summary is already visible in place.
  function summaryShowsInPlace(form) {
    var panel = form.closest(".article-inline") || form.closest(".reading-pane");
    var panelUrl = form.getAttribute("data-panel-url");
    if (panel && panelUrl) return true;
    return !form.closest("[data-article-card]");
  }

  function finishSummary(form) {
    var panel = form.closest(".article-inline") || form.closest(".reading-pane");
    var panelUrl = form.getAttribute("data-panel-url");
    if (panel && panelUrl) {
      return loadPanel(panel, panelUrl, null).then(function () {
        var panelArticle = panel.querySelector("[data-panel-article]");
        var card = panelArticle ? document.getElementById("card-" + panelArticle.getAttribute("data-panel-article")) : null;
        syncCardState(card, panelArticle);
      });
    }
    var card = form.closest("[data-article-card]");
    if (card) {
      var articleId = card.getAttribute("data-article-id");
      var link = document.createElement("a");
      link.href = "/articles/" + articleId;
      link.className = "btn summary-card-link";
      link.textContent = "View AI summary";
      form.replaceWith(link);
      return Promise.resolve();
    }
    window.location.reload();
    return new Promise(function () {});
  }

  // The live status text lives outside the summarize form itself (so a long
  // message never resizes the compact action row): the reader's reserved
  // "AI summary" slot, or the list card's status line below its action row.
  function summaryProgressElement(form) {
    var inline = form.querySelector("[data-summary-progress]");
    if (inline) return inline;
    var scope = form.closest("[data-reader-surface]") || form.closest("[data-article-card]");
    return scope ? scope.querySelector("[data-summary-progress]") : null;
  }

  function startSummaryPolling(form, options) {
    options = options || {};
    if (form._rustyFeedSummaryPromise) return form._rustyFeedSummaryPromise;
    var statusUrl = form.getAttribute("data-summary-status-url");
    if (!statusUrl) return Promise.resolve();
    var progress = summaryProgressElement(form);
    var deadline = Date.now() + (5 * 60 * 1000);
    var attempts = 0;
    // Resuming on page load stays silent until we confirm a job is actually in
    // flight, so a freshly opened article never flashes a disabled button or a
    // redundant idle status line underneath it.
    var active = !options.resume;
    if (active) setPending(form, true);

    function activate() {
      if (!active) {
        active = true;
        setPending(form, true);
      }
    }

    form._rustyFeedSummaryPromise = new Promise(function (resolve) {
      function finish(message, error) {
        if (progress) {
          progress.textContent = message;
          progress.classList.toggle("error", Boolean(error));
        }
        if (error) announce(message, true);
        resolve();
      }

      function poll() {
        if (!form.isConnected) {
          resolve();
          return;
        }
        fetch(statusUrl, {
          credentials: "same-origin",
          headers: { "X-Requested-With": "fetch" },
          cache: "no-store",
        })
          .then(function (response) {
            if (!response.ok) throw new Error("summary status failed");
            return response.json();
          })
          .then(function (result) {
            if (result.state === "idle") {
              if (progress) {
                progress.textContent = "";
                progress.classList.remove("error");
              }
              resolve();
              return;
            }
            if (result.state === "ready") {
              activate();
              // If the summary is about to appear right here (reader/panel),
              // showing it is enough — a toast would just repeat the news.
              if (!summaryShowsInPlace(form)) {
                announce("AI summary ready.", false);
              }
              finishSummary(form).then(resolve, function () {
                finish("The summary is ready, but this view could not refresh. Reload the page.", true);
              });
              return;
            }
            if (result.state === "failed") {
              activate();
              finish(result.message || "The AI summary could not be generated.", true);
              return;
            }
            // queued / running / retrying: an active job is in flight.
            activate();
            if (progress) {
              progress.textContent = result.message || "Generating the AI summary…";
              progress.classList.remove("error");
            }
            attempts += 1;
            if (Date.now() >= deadline) {
              finish("The summary is taking longer than expected. It remains in the priority queue.", false);
              return;
            }
            window.setTimeout(poll, Math.min(250 + attempts * 150, 2000));
          })
          .catch(function () {
            // A quiet resume probe should not surface transient errors.
            if (!active) {
              resolve();
              return;
            }
            attempts += 1;
            if (Date.now() >= deadline) {
              finish("Live status is unavailable. Reload later or ask an administrator to check the queue.", true);
              return;
            }
            window.setTimeout(poll, Math.min(500 + attempts * 250, 3000));
          });
      }
      poll();
    }).finally(function () {
      delete form._rustyFeedSummaryPromise;
      if (form.isConnected && active) setPending(form, false);
    });
    return form._rustyFeedSummaryPromise;
  }

  function initializeSummaryActions(root) {
    var forms = [];
    if (root.matches && root.matches(".summary-action[data-action='summarize']")) forms.push(root);
    Array.prototype.push.apply(forms, root.querySelectorAll(".summary-action[data-action='summarize']"));
    forms.forEach(function (form) {
      if (form.dataset.summaryInitialized === "true") return;
      form.dataset.summaryInitialized = "true";
      startSummaryPolling(form, { resume: true }).catch(function () {});
    });
  }

  function initializeAdminQueueRefresh() {
    var snapshot = document.querySelector("[data-admin-queue-snapshot]");
    var button = document.querySelector("[data-admin-queue-refresh]");
    var refreshStatus = document.querySelector("[data-admin-refresh-status]");
    if (!snapshot || !button) return;
    var pending = false;

    function refresh(manual) {
      if (pending || document.visibilityState === "hidden") return;
      if (!manual && snapshot.contains(document.activeElement)) return;
      pending = true;
      button.disabled = true;
      if (refreshStatus) refreshStatus.textContent = "Updating…";
      fetch(window.location.pathname, {
        credentials: "same-origin",
        headers: { "X-Requested-With": "fetch" },
        cache: "no-store",
      })
        .then(function (response) {
          if (!response.ok) throw new Error("queue refresh failed");
          return response.text();
        })
        .then(function (html) {
          var parsed = new DOMParser().parseFromString(html, "text/html");
          var replacement = parsed.querySelector("[data-admin-queue-snapshot]");
          if (!replacement) throw new Error("queue snapshot missing");
          var disclosureState = {};
          var horizontalScroll = {};
          Array.prototype.forEach.call(snapshot.querySelectorAll("[data-admin-section]"), function (section) {
            var key = section.getAttribute("data-admin-section");
            if (!key) return;
            disclosureState[key] = section.open;
            var table = section.querySelector(".table-wrap");
            if (table) horizontalScroll[key] = table.scrollLeft;
          });
          var pageScroll = window.scrollY;
          snapshot.innerHTML = replacement.innerHTML;
          Array.prototype.forEach.call(snapshot.querySelectorAll("[data-admin-section]"), function (section) {
            var key = section.getAttribute("data-admin-section");
            if (!key || disclosureState[key] === undefined) return;
            section.open = disclosureState[key];
            var table = section.querySelector(".table-wrap");
            if (table && horizontalScroll[key] !== undefined) table.scrollLeft = horizontalScroll[key];
          });
          window.scrollTo({ top: pageScroll, behavior: "auto" });
          if (refreshStatus) refreshStatus.textContent = "Updated just now · refreshes every 10 seconds";
        })
        .catch(function () {
          if (refreshStatus) refreshStatus.textContent = "Update failed · retrying automatically";
        })
        .finally(function () {
          pending = false;
          button.disabled = false;
        });
    }

    button.addEventListener("click", function () { refresh(true); });
    window.setInterval(function () { refresh(false); }, 10000);
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

  function initializeBulkFeedActions() {
    var form = document.getElementById("bulk-feed-actions");
    if (!form) return;
    var inputs = Array.prototype.slice.call(document.querySelectorAll("[data-feed-select][form='bulk-feed-actions']"));
    var selectAll = form.querySelector("[data-bulk-select-all]");
    var count = form.querySelector("[data-bulk-selection-count]");
    var actions = form.querySelectorAll("[data-bulk-action]");
    var maximum = Number(form.getAttribute("data-bulk-max")) || 100;
    function selectedCount() {
      return inputs.filter(function (input) { return input.checked; }).length;
    }
    function sync() {
      var selected = selectedCount();
      if (count) count.textContent = selected ? selected + " feed" + (selected === 1 ? "" : "s") + " selected." : "Select feeds to use bulk actions.";
      if (selectAll) {
        selectAll.checked = inputs.length > 0 && selected === inputs.length;
        selectAll.indeterminate = selected > 0 && selected < inputs.length;
      }
      Array.prototype.forEach.call(actions, function (action) {
        action.disabled = selected === 0;
      });
    }
    inputs.forEach(function (input) {
      input.addEventListener("change", function () {
        if (input.checked && selectedCount() > maximum) {
          input.checked = false;
          announce("Bulk actions support up to " + maximum + " feeds at once.", true);
        }
        sync();
      });
    });
    if (selectAll) {
      selectAll.addEventListener("change", function () {
        inputs.forEach(function (input, index) { input.checked = selectAll.checked && index < maximum; });
        if (selectAll.checked && inputs.length > maximum) {
          announce("Selected the first " + maximum + " feeds. Refine the selection to choose others.", false);
        }
        sync();
      });
    }
    sync();
  }

  var draggingFeedId = null;
  var draggingFeedRow = null;
  function clearFeedDragTargets() {
    document.documentElement.classList.remove("feed-drag-active");
    Array.prototype.forEach.call(document.querySelectorAll("[data-feed-drop]"), function (target) {
      target.classList.remove("drag-over");
      target.removeAttribute("aria-busy");
    });
  }

  function initializeFeedDragAndDrop() {
    var csrf = document.documentElement.getAttribute("data-csrf-token");
    var handles = document.querySelectorAll("[data-feed-drag]");
    var targets = document.querySelectorAll("[data-feed-drop]");
    if (!csrf || !handles.length || !targets.length) return;
    Array.prototype.forEach.call(handles, function (handle) {
      if (handle.dataset.feedDragInitialized === "true") return;
      handle.dataset.feedDragInitialized = "true";
      handle.addEventListener("dragstart", function (event) {
        draggingFeedId = handle.getAttribute("data-feed-id");
        draggingFeedRow = handle.closest("[data-feed-row],[data-article-card],[data-sidebar-feed-row]");
        if (!draggingFeedId || !event.dataTransfer) return;
        document.documentElement.classList.add("feed-drag-active");
        event.dataTransfer.effectAllowed = "move";
        event.dataTransfer.setData("text/plain", draggingFeedId);
        if (draggingFeedRow) draggingFeedRow.classList.add("dragging");
      });
      handle.addEventListener("dragend", function () {
        if (draggingFeedRow) draggingFeedRow.classList.remove("dragging");
        draggingFeedId = null;
        draggingFeedRow = null;
        clearFeedDragTargets();
      });
    });
    Array.prototype.forEach.call(targets, function (target) {
      if (target.dataset.feedDropInitialized === "true") return;
      target.dataset.feedDropInitialized = "true";
      target.addEventListener("dragover", function (event) {
        if (!draggingFeedId) return;
        event.preventDefault();
        if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
        target.classList.add("drag-over");
      });
      target.addEventListener("dragleave", function (event) {
        if (!target.contains(event.relatedTarget)) target.classList.remove("drag-over");
      });
      target.addEventListener("drop", function (event) {
        event.preventDefault();
        var feedId = draggingFeedId || (event.dataTransfer && event.dataTransfer.getData("text/plain"));
        if (!feedId) return;
        var body = new URLSearchParams();
        body.set("csrf_token", csrf);
        body.set("folder_id", target.getAttribute("data-folder-id") || "");
        target.setAttribute("aria-busy", "true");
        fetch("/feeds/" + encodeURIComponent(feedId) + "/folder", {
          method: "POST",
          body: body,
          headers: { "X-Requested-With": "fetch" },
          credentials: "same-origin",
        })
          .then(function (response) {
            if (!response.ok) throw new Error("move failed");
            announce("Feed moved to " + (target.getAttribute("data-folder-label") || "the selected folder") + ".", false);
            window.setTimeout(function () { window.location.reload(); }, 120);
          })
          .catch(function () {
            target.removeAttribute("aria-busy");
            announce("Could not move that feed. Please try the folder picker.", true);
          })
          .finally(clearFeedDragTargets);
      });
    });
  }

  function initializeFeedMoveDialog() {
    var dialog = document.querySelector("[data-move-feed-dialog]");
    var form = dialog ? dialog.querySelector("[data-move-feed-form]") : null;
    if (!dialog || !form || typeof dialog.showModal !== "function") return;
    var title = dialog.querySelector("[data-move-feed-title]");
    var movedByDrag = false;

    Array.prototype.forEach.call(document.querySelectorAll("[data-feed-drag]"), function (handle) {
      if (handle.dataset.feedMoveInitialized === "true") return;
      handle.dataset.feedMoveInitialized = "true";
      handle.addEventListener("dragstart", function () { movedByDrag = true; });
      handle.addEventListener("dragend", function () {
        window.setTimeout(function () { movedByDrag = false; }, 0);
      });
      handle.addEventListener("click", function () {
        if (movedByDrag) return;
        var feedId = handle.getAttribute("data-feed-id");
        var feedName = handle.getAttribute("data-feed-name") || "source feed";
        if (!feedId) return;
        form.action = "/feeds/" + encodeURIComponent(feedId) + "/folder";
        form.setAttribute("data-feed-id", feedId);
        if (title) title.textContent = "Move " + feedName;
        dialog.showModal();
      });
    });

    if (dialog.dataset.feedMoveDialogInitialized === "true") return;
    dialog.dataset.feedMoveDialogInitialized = "true";
    Array.prototype.forEach.call(dialog.querySelectorAll("[data-move-feed-close]"), function (button) {
      button.addEventListener("click", function () { if (dialog.open) dialog.close(); });
    });
    dialog.addEventListener("click", function (event) {
      if (event.target === dialog) dialog.close();
    });
    form.addEventListener("submit", function (event) {
      event.preventDefault();
      if (form.dataset.pending === "true") return;
      setPending(form, true);
      fetch(form.action, {
        method: "POST",
        body: encodedForm(form),
        headers: { "X-Requested-With": "fetch" },
        credentials: "same-origin",
      }).then(function (response) {
        if (!response.ok) throw new Error("feed move failed");
        var folder = form.querySelector("[data-move-feed-folder]");
        var destination = folder && folder.selectedOptions.length ? folder.selectedOptions[0].textContent : "the selected folder";
        dialog.close();
        announce("Feed moved to " + destination + ".", false);
        window.setTimeout(function () { window.location.reload(); }, 120);
      }).catch(function () {
        announce("Could not move that feed. Please try again.", true);
      }).finally(function () { setPending(form, false); });
    });
  }

  function initializeFolderCollapse() {
    var buttons = document.querySelectorAll("[data-folder-collapse]");
    if (!buttons.length) return;

    /** @param {string} folderId @param {boolean} collapsed */
    function apply(folderId, collapsed) {
      Array.prototype.forEach.call(buttons, function (button) {
        if (button.getAttribute("data-folder-collapse") !== folderId) return;
        var folder = button.closest(".folder-tree-node");
        var feeds = folder ? folder.querySelector(".folder-tree-feeds") : null;
        var name = folder ? folder.getAttribute("data-folder-label") || "folder" : "folder";
        if (folder) folder.classList.toggle("folder-collapsed", collapsed);
        button.setAttribute("aria-expanded", String(!collapsed));
        button.setAttribute("aria-label", (collapsed ? "Expand " : "Reduce ") + name);
        var glyph = button.querySelector("span");
        if (glyph) glyph.textContent = collapsed ? "›" : "⌄";
        if (feeds) feeds.hidden = collapsed;
      });
    }

    Array.prototype.forEach.call(buttons, function (button) {
      var folderId = button.getAttribute("data-folder-collapse");
      var folder = button.closest(".folder-tree-node");
      var initial = folder && folder.getAttribute("data-folder-collapsed") === "true";
      apply(folderId, initial);
      button.addEventListener("click", function () {
        var collapsed = button.getAttribute("aria-expanded") === "true";
        apply(folderId, collapsed);
        var csrf = document.documentElement.getAttribute("data-csrf-token");
        if (!csrf) return;
        var body = new URLSearchParams();
        body.set("csrf_token", csrf);
        body.set("collapsed", String(collapsed));
        fetch("/feeds/categories/" + encodeURIComponent(folderId) + "/collapse", {
          method: "POST",
          body: body,
          headers: { "X-Requested-With": "fetch" },
          credentials: "same-origin",
        }).then(function (response) {
          if (!response.ok) throw new Error("folder state save failed");
        }).catch(function () {
          apply(folderId, !collapsed);
          announce("Could not save that folder state.", true);
        });
      });
    });
  }

  function initializeSidebarQuickTools() {
    var root = document.documentElement;
    var toggles = document.querySelectorAll("[data-sidebar-quick-toggle]");
    if (!toggles.length) return;
    // Seed from the server-rendered class so the pinned state survives navigation.
    var active = root.classList.contains("sidebar-quick-tools-active");
    /** @param {boolean} next */
    function apply(next) {
      active = next;
      root.classList.toggle("sidebar-quick-tools-active", active);
      Array.prototype.forEach.call(toggles, function (toggle) {
        toggle.setAttribute("aria-pressed", String(active));
      });
    }
    apply(active);
    Array.prototype.forEach.call(toggles, function (toggle) {
      toggle.addEventListener("click", function () {
        var next = !active;
        apply(next);
        announce(next ? "Quick feed tools pinned." : "Quick feed tools hidden.", false);
        var url = root.getAttribute("data-layout-url");
        var csrf = root.getAttribute("data-csrf-token");
        if (!url || !csrf) return;
        var body = new URLSearchParams();
        body.set("csrf_token", csrf);
        body.set("quick_tools_visible", String(next));
        fetch(url, {
          method: "POST",
          body: body,
          headers: { "X-Requested-With": "fetch" },
          credentials: "same-origin",
        }).catch(function () {
          // Non-fatal: the toggle still works for this page load.
        });
      });
    });
  }

  function initializeImageLightbox() {
    var dialog = document.querySelector("[data-image-lightbox]");
    if (!dialog || typeof dialog.showModal !== "function") return;
    var img = dialog.querySelector("[data-image-lightbox-img]");
    var openLink = dialog.querySelector("[data-image-lightbox-open]");

    function open(src, alt) {
      if (!src) return;
      if (img) {
        img.src = src;
        img.alt = alt || "";
      }
      if (openLink) openLink.href = src;
      if (!dialog.open) dialog.showModal();
    }
    function reset() {
      if (img) img.removeAttribute("src");
    }

    document.addEventListener("click", function (event) {
      var trigger = event.target && event.target.closest ? event.target.closest("[data-image-modal]") : null;
      if (!trigger) return;
      var src = trigger.getAttribute("data-image-src") || trigger.getAttribute("href");
      if (!src) return;
      event.preventDefault();
      open(src, trigger.getAttribute("data-image-alt") || trigger.getAttribute("alt"));
    });
    Array.prototype.forEach.call(dialog.querySelectorAll("[data-image-lightbox-close]"), function (button) {
      button.addEventListener("click", function () {
        if (dialog.open) dialog.close();
      });
    });
    // A click on the backdrop (the dialog box padding, outside the image) closes.
    dialog.addEventListener("click", function (event) {
      if (event.target === dialog) dialog.close();
    });
    dialog.addEventListener("close", reset);
  }

  function initializeIntegrationHints() {
    var select = document.querySelector("[data-provider-select]");
    var hint = document.querySelector("[data-provider-hint]");
    if (!select || !hint) return;
    var endpoint = hint.querySelector("[data-hint-endpoint]");
    var credential = hint.querySelector("[data-hint-credential]");
    var payload = hint.querySelector("[data-hint-payload]");
    function update() {
      var option = select.options[select.selectedIndex];
      if (!option) return;
      if (endpoint) endpoint.textContent = option.getAttribute("data-endpoint-hint") || "";
      if (credential) credential.textContent = option.getAttribute("data-credential-hint") || "";
      if (payload) payload.textContent = option.getAttribute("data-payload") || "";
    }
    select.addEventListener("change", update);
    update();
  }

  function initializeProviderCopy() {
    var form = document.querySelector("[data-provider-form]");
    if (!form) return;
    function setField(name, value) {
      var field = form.querySelector("[name='" + name + "']");
      if (!field) return null;
      if (field.tagName === "SELECT") {
        var match = Array.prototype.some.call(field.options, function (option) {
          return option.value === value;
        });
        if (match) field.value = value;
      } else {
        field.value = value;
      }
      return field;
    }
    Array.prototype.forEach.call(document.querySelectorAll("[data-provider-copy]"), function (button) {
      button.addEventListener("click", function () {
        setField("name", button.getAttribute("data-name") || "");
        setField("provider", button.getAttribute("data-provider") || "");
        setField("model", button.getAttribute("data-model") || "");
        setField("base_url", button.getAttribute("data-base-url") || "");
        // Never copy a key: the user must supply their own.
        var key = setField("api_key", "");
        form.scrollIntoView({ behavior: "smooth", block: "start" });
        if (key && typeof key.focus === "function") key.focus();
        announce("Copied provider details. Enter your own API key to save.", false);
      });
    });
  }

  function initializeQuickSettingsDialogs() {
    var contentDialog = document.querySelector("[data-content-settings-dialog]");
    var contentForm = contentDialog ? contentDialog.querySelector("[data-content-settings-form]") : null;
    var tagsDialog = document.querySelector("[data-tags-settings-dialog]");
    var tagsForm = tagsDialog ? tagsDialog.querySelector("[data-tags-settings-form]") : null;
    if (!contentDialog || !contentForm || !tagsDialog || !tagsForm || typeof contentDialog.showModal !== "function") return;

    /** @param {HTMLDialogElement} dialog */
    function close(dialog) {
      if (dialog.open) dialog.close();
    }

    Array.prototype.forEach.call(document.querySelectorAll("[data-feed-content-trigger]"), function (trigger) {
      trigger.addEventListener("click", function () {
        var feedId = trigger.getAttribute("data-feed-id");
        var mode = trigger.getAttribute("data-content-mode") || "rss";
        var lifetime = trigger.getAttribute("data-content-lifetime") || "7";
        var title = contentDialog.querySelector("[data-content-settings-title]");
        contentForm.action = "/feeds/" + encodeURIComponent(feedId) + "/content-settings";
        contentForm.setAttribute("data-feed-id", feedId);
        var modeControl = contentForm.querySelector("input[name='content_mode'][value='" + mode + "']");
        var lifetimeControl = contentForm.querySelector("select[name='content_lifetime']");
        if (modeControl) modeControl.checked = true;
        if (lifetimeControl) lifetimeControl.value = lifetime;
        if (title) title.textContent = "Content settings · " + (trigger.getAttribute("data-feed-name") || "feed");
        contentDialog.showModal();
      });
    });

    Array.prototype.forEach.call(document.querySelectorAll("[data-feed-tags-trigger]"), function (trigger) {
      trigger.addEventListener("click", function () {
        var feedId = trigger.getAttribute("data-feed-id");
        var title = tagsDialog.querySelector("[data-tags-settings-title]");
        var tags = tagsForm.querySelector("[data-tags-settings-input]");
        var folder = tagsForm.querySelector("[data-tags-folder-id]");
        tagsForm.action = "/feeds/" + encodeURIComponent(feedId) + "/organization";
        tagsForm.setAttribute("data-feed-id", feedId);
        if (tags) tags.value = trigger.getAttribute("data-feed-tags") || "";
        if (folder) folder.value = trigger.getAttribute("data-feed-folder-id") || "";
        if (title) title.textContent = "Tags · " + (trigger.getAttribute("data-feed-name") || "feed");
        tagsDialog.showModal();
      });
    });

    /** @param {HTMLFormElement} form @param {HTMLDialogElement} dialog @param {string} message */
    function submit(form, dialog, message) {
      form.addEventListener("submit", function (event) {
        event.preventDefault();
        if (form.dataset.pending === "true") return;
        setPending(form, true);
        fetch(form.action, {
          method: "POST",
          body: encodedForm(form),
          headers: { "X-Requested-With": "fetch" },
          credentials: "same-origin",
        })
          .then(function (response) {
            if (!response.ok) throw new Error("quick settings save failed");
            var feedId = form.getAttribute("data-feed-id");
            Array.prototype.forEach.call(document.querySelectorAll("[data-feed-id='" + feedId + "']"), function (trigger) {
              if (form === contentForm && trigger.hasAttribute("data-feed-content-trigger")) {
                var mode = contentForm.querySelector("input[name='content_mode']:checked");
                var lifetime = contentForm.querySelector("select[name='content_lifetime']");
                if (mode) trigger.setAttribute("data-content-mode", mode.value);
                if (lifetime) trigger.setAttribute("data-content-lifetime", lifetime.value);
              }
              if (form === tagsForm && trigger.hasAttribute("data-feed-tags-trigger")) {
                var tags = tagsForm.querySelector("[data-tags-settings-input]");
                if (tags) trigger.setAttribute("data-feed-tags", tags.value);
              }
            });
            close(dialog);
            announce(message, false);
          })
          .catch(function () {
            announce("Could not save those feed settings.", true);
          })
          .finally(function () { setPending(form, false); });
      });
    }

    submit(contentForm, contentDialog, "Content settings saved.");
    submit(tagsForm, tagsDialog, "Tags saved.");
    Array.prototype.forEach.call(document.querySelectorAll("[data-quick-dialog-close]"), function (button) {
      button.addEventListener("click", function () {
        var dialog = button.closest("dialog");
        if (dialog) close(dialog);
      });
    });
    [contentDialog, tagsDialog].forEach(function (dialog) {
      dialog.addEventListener("click", function (event) {
        if (event.target === dialog) close(dialog);
      });
    });
  }

  function initializeArticleSendDialog() {
    var capability = document.createElement("dialog");
    if (typeof capability.showModal !== "function") return;

    document.addEventListener("click", function (event) {
      var trigger = event.target.closest ? event.target.closest("[data-article-send-trigger]") : null;
      if (!trigger || (event.button !== undefined && event.button !== 0) || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
      var modalUrl = trigger.getAttribute("data-modal-url");
      if (!modalUrl || trigger.getAttribute("aria-busy") === "true") return;
      event.preventDefault();
      trigger.setAttribute("aria-busy", "true");

      fetch(modalUrl, {
        headers: { "X-Requested-With": "fetch" },
        credentials: "same-origin",
      })
        .then(function (response) {
          if (!response.ok) throw new Error("send dialog load failed");
          return response.text();
        })
        .then(function (markup) {
          var holder = document.createElement("div");
          holder.innerHTML = markup.trim();
          var dialog = holder.firstElementChild;
          if (!dialog || dialog.tagName !== "DIALOG" || !dialog.hasAttribute("data-article-send-dialog")) {
            throw new Error("invalid send dialog response");
          }
          document.body.appendChild(dialog);

          function close() {
            if (dialog.open) dialog.close();
          }
          Array.prototype.forEach.call(dialog.querySelectorAll("[data-article-send-close]"), function (button) {
            button.addEventListener("click", close);
          });
          dialog.addEventListener("click", function (dialogEvent) {
            if (dialogEvent.target === dialog) close();
          });
          dialog.addEventListener("close", function () {
            dialog.remove();
            if (trigger.isConnected) trigger.focus();
          }, { once: true });

          var form = dialog.querySelector("[data-article-send-form]");
          if (form) {
            form.addEventListener("submit", function (submitEvent) {
              submitEvent.preventDefault();
              if (form.dataset.pending === "true") return;
              setPending(form, true);
              fetch(form.action, {
                method: "POST",
                body: encodedForm(form),
                headers: { "X-Requested-With": "fetch" },
                credentials: "same-origin",
              })
                .then(function (response) {
                  if (!response.ok) throw new Error("article send failed");
                  close();
                  announce(form.getAttribute("data-success-message") || "Article delivery queued.", false);
                })
                .catch(function () {
                  announce("The article could not be queued for delivery. Please try again.", true);
                })
                .finally(function () { setPending(form, false); });
            });
          }

          dialog.showModal();
          var firstControl = dialog.querySelector("[data-article-send-destination], a[href], button:not([disabled])");
          if (firstControl) firstControl.focus();
        })
        .catch(function () {
          announce("The destination picker could not be opened. Please try again.", true);
        })
        .finally(function () {
          trigger.removeAttribute("aria-busy");
        });
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
    if (panel._rustyFeedAbort) panel._rustyFeedAbort.abort();
    var controller = window.AbortController ? new AbortController() : null;
    panel._rustyFeedAbort = controller;
    var request = String((Number(panel.getAttribute("data-panel-request")) || 0) + 1);
    panel.setAttribute("data-panel-request", request);
    reportReadingWithin(panel);
    panel.setAttribute("aria-busy", "true");
    panel.innerHTML = '<div class="reading-pane-empty">Loading article…</div>';
    return fetch(url, {
      headers: { "X-Requested-With": "fetch" },
      credentials: "same-origin",
      signal: controller ? controller.signal : undefined,
    })
      .then(function (response) {
        if (!response.ok) throw new Error("panel failed");
        return response.text();
      })
      .then(function (html) {
        if (panel.getAttribute("data-panel-request") !== request) return false;
        panel.innerHTML = html;
        panel.removeAttribute("aria-busy");
        initializeReadingTracking(panel);
        initializePendingContent(panel);
        initializeSummaryActions(panel);
        initializeDefaultLiveViews(panel);
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
        return true;
      })
      .catch(function (error) {
        if (panel.getAttribute("data-panel-request") !== request || (error && error.name === "AbortError")) return false;
        panel.removeAttribute("aria-busy");
        throw error;
      });
  }

  /**
   * Switch an article between its stored reader content and isolated Live View.
   * @param {HTMLElement} control
   * @param {boolean=} closeOnly
   */
  function toggleLiveView(control, closeOnly) {
    var article = control.closest("article");
    var reader = article ? article.querySelector("[data-reader-view-content]") : null;
    var stored = reader ? reader.querySelector("[data-stored-content]") || reader : null;
    var live = article ? article.querySelector("[data-live-view]") : null;
    var frame = live ? live.querySelector("[data-live-view-frame]") : null;
    var liveStatus = live ? live.querySelector("[data-live-view-status]") : null;
    var button = article ? article.querySelector("[data-live-view-toggle],[data-content-live-control]") : null;
    if (!reader || !stored || !live || !frame) return;
    var showLive = !closeOnly && !article.classList.contains("live-view-active");
    if (showLive) {
      var ticketUrl = frame.getAttribute("data-live-ticket-url");
      var source = frame.getAttribute("data-live-src");
      if (!ticketUrl && !source) return;
      article.classList.add("live-view-active");
      live.hidden = false;
      stored.hidden = true;
      live.setAttribute("aria-busy", "true");
      if (liveStatus) {
        liveStatus.hidden = false;
        liveStatus.classList.remove("error");
        liveStatus.textContent = "Loading the current publisher page…";
      }
      if (frame._rustyFeedLiveController) frame._rustyFeedLiveController.abort();
      var controller = window.AbortController ? new AbortController() : null;
      frame._rustyFeedLiveController = controller;
      var listenerOptions = controller ? { once: true, signal: controller.signal } : { once: true };
      var loadTimeout = window.setTimeout(function () {
        if (live.hidden || !liveStatus) return;
        live.removeAttribute("aria-busy");
        liveStatus.hidden = false;
        liveStatus.classList.add("error");
        liveStatus.textContent = "The publisher is taking longer than expected. You can keep waiting or open the original page.";
      }, 35000);
      frame._rustyFeedLiveTimeout = loadTimeout;
      frame.addEventListener("load", function () {
        window.clearTimeout(loadTimeout);
        if (live.hidden) return;
        live.removeAttribute("aria-busy");
        if (liveStatus) liveStatus.hidden = true;
        announce("Live publisher view is ready.", false);
      }, listenerOptions);
      frame.addEventListener("error", function () {
        window.clearTimeout(loadTimeout);
        if (live.hidden) return;
        live.removeAttribute("aria-busy");
        if (liveStatus) {
          liveStatus.hidden = false;
          liveStatus.classList.add("error");
          liveStatus.textContent = "Live View could not load. The stored reader view remains available.";
        }
        announce("Live View could not load.", true);
      }, listenerOptions);
      if (ticketUrl) {
        fetch(ticketUrl, {
          credentials: "same-origin",
          headers: { "X-Requested-With": "fetch" },
          signal: controller ? controller.signal : undefined,
        })
          .then(function (response) {
            if (!response.ok) throw new Error("live ticket request failed");
            return response.text();
          })
          .then(function (liveUrl) {
            if (live.hidden || !liveUrl) return;
            frame.setAttribute("src", liveUrl);
          })
          .catch(function (error) {
            if (error && error.name === "AbortError") return;
            window.clearTimeout(loadTimeout);
            live.removeAttribute("aria-busy");
            if (liveStatus) {
              liveStatus.hidden = false;
              liveStatus.classList.add("error");
              liveStatus.textContent = "Live View could not start. The stored reader view remains available.";
            }
            announce("Live View could not start.", true);
          });
      } else {
        frame.setAttribute("src", source);
      }
    } else {
      if (frame._rustyFeedLiveController) frame._rustyFeedLiveController.abort();
      window.clearTimeout(frame._rustyFeedLiveTimeout);
      frame.removeAttribute("src");
      live.hidden = true;
      live.removeAttribute("aria-busy");
      stored.hidden = false;
      article.classList.remove("live-view-active");
      if (liveStatus) {
        liveStatus.hidden = false;
        liveStatus.classList.remove("error");
        liveStatus.textContent = "Loading the current publisher page…";
      }
    }
    if (button) {
      button.setAttribute("aria-expanded", String(showLive));
      if (button.hasAttribute("data-live-view-toggle")) button.textContent = showLive ? "Reader view" : "Live view";
    }
    announce(
      showLive
        ? "Loading the current publisher page in an isolated Live View."
        : "Reader view restored.",
      false
    );
    if (!showLive && control.hasAttribute("data-live-view-close") && button) button.focus();
  }

  function initializeDefaultLiveViews(root) {
    Array.prototype.forEach.call(root.querySelectorAll("[data-reader-surface][data-content-mode-current='live']"), function (article) {
      if (article.classList.contains("live-view-active")) return;
      var control = article.querySelector("[data-content-live-control]");
      if (control) toggleLiveView(control, false);
    });
  }

  document.addEventListener("click", function (event) {
    var control = event.target.closest ? event.target.closest("[data-live-view-toggle],[data-live-view-close]") : null;
    if (!control) return;
    event.preventDefault();
    toggleLiveView(control, control.hasAttribute("data-live-view-close"));
  });

  document.addEventListener("keydown", function (event) {
    if (event.key !== "Escape") return;
    var live = document.querySelector("[data-live-view]:not([hidden])");
    var close = live ? live.querySelector("[data-live-view-close]") : null;
    if (close) toggleLiveView(close, true);
  });

  document.addEventListener("click", function (event) {
    var link = event.target.closest ? event.target.closest("[data-content-view-link]") : null;
    if (!link || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    var panel = link.closest(".article-inline, .reading-pane");
    var panelUrl = link.getAttribute("data-content-panel-url");
    if (!panel || !panelUrl) return;
    event.preventDefault();
    var article = link.closest("article.live-view-active");
    var close = article ? article.querySelector("[data-live-view-close]") : null;
    if (close) toggleLiveView(close, true);
    loadPanel(panel, panelUrl, null).catch(function () {
      window.location.assign(link.href);
    });
  });

  document.addEventListener("submit", function (event) {
    var form = event.target;
    if (!form.matches || !form.matches("[data-content-default-form]")) return;
    event.preventDefault();
    if (form.dataset.pending === "true") return;
    setPending(form, true);
    fetch(form.action, {
      method: "POST",
      body: encodedForm(form),
      headers: { "X-Requested-With": "fetch" },
      credentials: "same-origin",
    }).then(function (response) {
      if (!response.ok) throw new Error("content mode update failed");
      var panel = form.closest(".article-inline, .reading-pane");
      var panelUrl = form.getAttribute("data-content-panel-url");
      if (panel && panelUrl) return loadPanel(panel, panelUrl, null);
      var next = form.querySelector("input[name='next']");
      window.location.assign(next ? next.value : window.location.href);
      return null;
    }).catch(function () {
      announce("The content source could not be changed. Please try again.", true);
      setPending(form, false);
    });
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

        if (form.getAttribute("data-action") === "summarize") {
          announce(form.getAttribute("data-success-message") || "AI summary queued.", false);
          return startSummaryPolling(form);
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
        if (form.getAttribute("data-action") !== "mark-all-read" && form.getAttribute("data-action") !== "summarize") {
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

  function initializeCatalogSelection() {
    function storageKey(element) {
      var owner = element ? element.getAttribute("data-catalog-storage-key") : "";
      return owner ? "rusty-feed.catalog-selection." + owner : "";
    }
    function readState(key) {
      if (!key) return { feeds: {}, returnUrl: "" };
      try {
        var parsed = JSON.parse(window.sessionStorage.getItem(key) || "{}");
        return {
          feeds: parsed.feeds && typeof parsed.feeds === "object" ? parsed.feeds : {},
          returnUrl: typeof parsed.returnUrl === "string" ? parsed.returnUrl : "",
        };
      } catch (_) {
        return { feeds: {}, returnUrl: "" };
      }
    }
    function writeState(key, state) {
      if (!key) return;
      try { window.sessionStorage.setItem(key, JSON.stringify(state)); } catch (_) {}
    }

    var back = document.querySelector("[data-catalog-back]");
    if (back) {
      var backState = readState(storageKey(back));
      if (/^\/catalog(?:\?|$)/.test(backState.returnUrl)) back.setAttribute("href", backState.returnUrl);
    }

    var form = document.querySelector(".catalog-selection");
    if (!form) return;
    var key = storageKey(form);
    var state = readState(key);
    var toggle = form.querySelector("[data-catalog-select-all]");
    var label = form.querySelector("[data-catalog-select-all-label]");
    var count = form.querySelector("[data-catalog-selection-count]");
    var clear = form.querySelector("[data-catalog-clear]");
    var submit = form.querySelector("button[type='submit']");
    function boxes() {
      // Only enabled checkboxes are selectable; disabled ones are already subscribed.
      // These already reflect the applied category/country/language filters.
      return Array.prototype.slice.call(form.querySelectorAll("input[name='feed_id']:not([disabled]):not([data-catalog-stored])"));
    }
    function modeFor(box) {
      var mode = form.querySelector("input[name='content_mode." + box.value + "']:checked");
      return mode ? mode.value : "rss";
    }
    Array.prototype.forEach.call(form.querySelectorAll("input[name='feed_id'][disabled]"), function (box) {
      delete state.feeds[box.value];
    });
    boxes().forEach(function (box) {
      var mode = state.feeds[box.value];
      if (!mode) return;
      box.checked = true;
      var choice = form.querySelector("input[name='content_mode." + box.value + "'][value='" + mode + "']");
      if (choice) choice.checked = true;
    });
    function sync() {
      var all = boxes();
      var checked = all.filter(function (box) { return box.checked; });
      var selected = Object.keys(state.feeds).length;
      if (toggle) {
        toggle.checked = all.length > 0 && checked.length === all.length;
        toggle.indeterminate = checked.length > 0 && checked.length < all.length;
        toggle.disabled = all.length === 0;
      }
      if (label) label.textContent = checked.length > 0 ? "Select all (" + checked.length + " shown selected)" : "Select all";
      if (count) count.textContent = selected === 0
        ? "No sources selected. Double-click a card to subscribe immediately."
        : selected + " selected across filters. Your choices stay selected while you browse and preview.";
      if (clear) clear.disabled = selected === 0;
      if (submit) submit.disabled = selected === 0;
      writeState(key, state);
    }
    if (clear) clear.addEventListener("click", function () {
      state.feeds = {};
      boxes().forEach(function (box) { box.checked = false; });
      sync();
      announce("Cleared the catalog selection.", false);
    });
    if (toggle) toggle.addEventListener("change", function () {
      var check = toggle.checked;
      boxes().forEach(function (box) {
        box.checked = check;
        if (check) state.feeds[box.value] = modeFor(box);
        else delete state.feeds[box.value];
      });
      sync();
      announce(check ? "Selected all filtered sources." : "Cleared the filtered selection.", false);
    });
    form.addEventListener("change", function (event) {
      if (!event.target) return;
      if (event.target.name === "feed_id") {
        if (event.target.checked) state.feeds[event.target.value] = modeFor(event.target);
        else delete state.feeds[event.target.value];
      } else if (event.target.name && event.target.name.indexOf("content_mode.") === 0) {
        var feedId = event.target.name.slice("content_mode.".length);
        if (state.feeds[feedId]) state.feeds[feedId] = event.target.value;
      }
      sync();
    });
    form.addEventListener("click", function (event) {
      var preview = event.target.closest ? event.target.closest(".catalog-preview-link") : null;
      if (!preview) return;
      state.returnUrl = window.location.pathname + window.location.search;
      writeState(key, state);
    });
    form.addEventListener("dblclick", function (event) {
      var card = event.target.closest ? event.target.closest("[data-catalog-selectable]") : null;
      if (!card || event.target.closest("a, button, input, select, textarea, label, summary, [contenteditable], [role='button']")) return;
      var checkbox = card.querySelector("input[name='feed_id']");
      if (!checkbox || checkbox.disabled) return;
      event.preventDefault();
      if (!checkbox.checked) {
        checkbox.checked = true;
        checkbox.dispatchEvent(new Event("change", { bubbles: true }));
      }
      announce("Subscribing to the selected sources.", false);
      form.requestSubmit(submit);
    });
    form.addEventListener("submit", function () {
      Array.prototype.forEach.call(form.querySelectorAll("[data-catalog-stored]"), function (input) { input.remove(); });
      var visible = {};
      boxes().forEach(function (box) { visible[box.value] = true; });
      Object.keys(state.feeds).forEach(function (feedId) {
        if (visible[feedId]) return;
        var feed = document.createElement("input");
        feed.type = "hidden";
        feed.name = "feed_id";
        feed.value = feedId;
        feed.setAttribute("data-catalog-stored", "true");
        form.appendChild(feed);
        var mode = document.createElement("input");
        mode.type = "hidden";
        mode.name = "content_mode." + feedId;
        mode.value = state.feeds[feedId];
        mode.setAttribute("data-catalog-stored", "true");
        form.appendChild(mode);
      });
    });
    sync();
  }

  function initializeOpmlPreview() {
    var form = document.querySelector("[data-opml-preview]");
    if (!form) return;
    var entries = function () {
      return Array.prototype.slice.call(form.querySelectorAll("[data-opml-entry]"));
    };
    var update = function () {
      var selected = entries().filter(function (entry) { return entry.checked; }).length;
      var count = form.querySelector("[data-opml-selection-count]");
      if (count) count.textContent = selected + " selected.";
      var submit = form.querySelector("[data-opml-import]");
      if (submit) submit.disabled = selected === 0;
      Array.prototype.forEach.call(form.querySelectorAll("[data-opml-group-toggle]"), function (toggle) {
        var group = document.getElementById(toggle.getAttribute("data-opml-group-id"));
        var groupEntries = group ? Array.prototype.slice.call(group.querySelectorAll("[data-opml-entry]")) : [];
        var checked = groupEntries.filter(function (entry) { return entry.checked; }).length;
        toggle.checked = groupEntries.length > 0 && checked === groupEntries.length;
        toggle.indeterminate = checked > 0 && checked < groupEntries.length;
      });
    };
    form.addEventListener("change", function (event) {
      var toggle = event.target.closest ? event.target.closest("[data-opml-group-toggle]") : null;
      if (toggle) {
        var group = document.getElementById(toggle.getAttribute("data-opml-group-id"));
        if (group) {
          Array.prototype.forEach.call(group.querySelectorAll("[data-opml-entry]"), function (entry) {
            entry.checked = toggle.checked;
          });
        }
      }
      update();
    });
    var selectAll = form.querySelector("[data-opml-select-all]");
    if (selectAll) selectAll.addEventListener("click", function () {
      entries().forEach(function (entry) { entry.checked = true; });
      update();
    });
    var selectNone = form.querySelector("[data-opml-select-none]");
    if (selectNone) selectNone.addEventListener("click", function () {
      entries().forEach(function (entry) { entry.checked = false; });
      update();
    });
    update();
  }

  function initializeLibraryTagNavigation() {
    var activeRequest = null;
    function updateSidebar(url) {
      var target = new URL(url, window.location.href);
      Array.prototype.forEach.call(document.querySelectorAll("#library-sidebar .nav-link, .mobile-library-context .nav-link"), function (link) {
        var candidate = new URL(link.href, window.location.href);
        var active = candidate.pathname === target.pathname && candidate.search === target.search;
        link.classList.toggle("active", active);
        if (active) link.setAttribute("aria-current", "page");
        else link.removeAttribute("aria-current");
      });
    }
    function load(url, push) {
      var current = document.querySelector("[data-library-results]");
      if (!current) {
        window.location.assign(url);
        return;
      }
      if (activeRequest) activeRequest.abort();
      activeRequest = window.AbortController ? new AbortController() : null;
      current.setAttribute("aria-busy", "true");
      fetch(url, {
        credentials: "same-origin",
        headers: { "Accept": "text/html", "X-Requested-With": "fetch" },
        signal: activeRequest ? activeRequest.signal : undefined,
      }).then(function (response) {
        if (!response.ok) throw new Error("tag navigation failed");
        return response.text();
      }).then(function (html) {
        var parsed = new DOMParser().parseFromString(html, "text/html");
        var incoming = parsed.querySelector("[data-library-results]");
        if (!incoming || !current.isConnected) throw new Error("tag results missing");
        reportReadingWithin(current);
        var liveClose = current.querySelector("article.live-view-active [data-live-view-close]");
        if (liveClose) toggleLiveView(liveClose, true);
        var replacement = document.importNode(incoming, true);
        var currentMobileContext = current.querySelector(".mobile-library-context");
        var incomingMobileContext = replacement.querySelector(".mobile-library-context");
        if (currentMobileContext && incomingMobileContext) incomingMobileContext.replaceWith(currentMobileContext);
        current.replaceWith(replacement);
        document.title = parsed.title;
        document.body.className = parsed.body.className;
        updateSidebar(url);
        initializeSummaryActions(replacement);
        initializeFeedDragAndDrop();
        initializeFeedMoveDialog();
        if (push) window.history.pushState({ libraryTag: true }, "", url);
        var heading = replacement.querySelector("h1");
        if (heading) {
          heading.setAttribute("tabindex", "-1");
          heading.focus({ preventScroll: true });
        }
        announce("Tag articles loaded.", false);
      }).catch(function (error) {
        if (error && error.name === "AbortError") return;
        window.location.assign(url);
      }).finally(function () {
        if (current.isConnected) current.removeAttribute("aria-busy");
      });
    }
    document.addEventListener("click", function (event) {
      var link = event.target.closest ? event.target.closest("[data-library-tag-nav]") : null;
      if (!link || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
      if (!document.querySelector("[data-library-results]")) return;
      event.preventDefault();
      load(link.href, true);
    });
    window.addEventListener("popstate", function () {
      if (window.location.pathname === "/articles") load(window.location.href, false);
    });
  }

  // Implicit reading signal: on an article page, report time spent when the
  // reader leaves, so personalization can learn from completion/bounce.
  initializeReadingTracking(document);
  initializePendingContent(document);
  initializeSummaryActions(document);
  initializeDefaultLiveViews(document);
  initializeLibraryTagNavigation();
  initializeFeedOrganizer();
  initializeBulkFeedActions();
  initializeFeedDragAndDrop();
  initializeFeedMoveDialog();
  initializeFolderCollapse();
  initializeSidebarQuickTools();
  initializeImageLightbox();
  initializeCatalogSelection();
  initializeProviderCopy();
  initializeIntegrationHints();
  initializeQuickSettingsDialogs();
  initializeArticleSendDialog();
  initializeAdminQueueRefresh();
  initializeSidebarToggle();
  initializePanelResize();
  initializeOpmlPreview();

  function defaultArticleUrls(link) {
    var article = new URL(link.href, window.location.href);
    article.search = "";
    article.hash = "";
    return {
      article: article.href,
      panel: article.href.replace(/\/$/, "") + "/panel",
    };
  }

  // Inline reading: expand article content in the list instead of navigating.
  document.addEventListener("click", function (event) {
    var link = event.target.closest ? event.target.closest("a.article-link") : null;
    if (!link || (event.button !== undefined && event.button !== 0) || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    var list = link.closest(".article-list");
    if (!list) return;
    var mode = list.getAttribute("data-reading-mode");
    if (mode !== "inline" && mode !== "side-panel") return;
    var workspace = list.closest(".reading-workspace");
    if (mode === "side-panel" && !sidePanelAvailable(workspace)) return;

    event.preventDefault();
    var urls = defaultArticleUrls(link);
    var panel;
    if (mode === "inline") {
      panel = link.closest(".article-card").querySelector(".article-inline");
      if (!panel.hidden && panel.getAttribute("data-panel-url") === urls.panel) {
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
      window.location.assign(urls.article);
      return;
    }
    panel.setAttribute("data-panel-url", urls.panel);
    loadPanel(panel, urls.panel, link)
      .then(function (opened) { if (opened) announce("Article opened with its feed default.", false); })
      .catch(function () { window.location.assign(urls.article); });
  });

  document.addEventListener("click", function (event) {
    if (event.defaultPrevented || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    var card = event.target.closest ? event.target.closest(".article-card") : null;
    if (!card || event.target.closest("a, button, input, select, textarea, label, form, summary, [contenteditable], [role='button']")) return;
    var list = card.closest(".article-list");
    if (!list || list.getAttribute("data-reading-mode") !== "side-panel") return;
    if (!sidePanelAvailable(list.closest(".reading-workspace"))) return;
    var link = card.querySelector("a.article-link");
    if (!link) return;
    event.preventDefault();
    link.click();
  });

  document.addEventListener("click", function (event) {
    var loadMedia = event.target.closest ? event.target.closest("[data-enclosure-load]") : null;
    if (!loadMedia) return;
    var loader = loadMedia.closest("[data-enclosure-loader]");
    var player = loader ? loader.querySelector("[data-enclosure-player]") : null;
    var source = player ? player.getAttribute("data-src") : null;
    if (!player || !source) return;
    player.setAttribute("src", source);
    player.removeAttribute("data-src");
    player.setAttribute("tabindex", "-1");
    player.hidden = false;
    loadMedia.remove();
    var explanation = loader.querySelector("span");
    if (explanation) explanation.remove();
    if (typeof player.load === "function" && player.tagName !== "IMG") player.load();
    player.focus();
    announce((loadMedia.getAttribute("data-enclosure-kind") || "Media") + " loaded.", false);
  });

  document.addEventListener("click", function (event) {
    var loadYouTube = event.target.closest ? event.target.closest("[data-youtube-load]") : null;
    if (!loadYouTube) return;
    var loader = loadYouTube.closest("[data-youtube-loader]");
    var frame = loader ? loader.querySelector("[data-youtube-frame]") : null;
    var consent = loader ? loader.querySelector("[data-youtube-consent]") : null;
    var player = loader ? loader.querySelector("[data-youtube-player]") : null;
    var status = loader ? loader.querySelector("[data-youtube-status]") : null;
    var source = frame ? frame.getAttribute("data-src") : null;
    if (!loader || !frame || !consent || !player || !source || loader.getAttribute("data-youtube-state") !== "idle") return;
    var attempt = String((Number(loader.getAttribute("data-youtube-attempt")) || 0) + 1);
    loader.setAttribute("data-youtube-attempt", attempt);
    loader.setAttribute("data-youtube-state", "loading");
    loadYouTube.setAttribute("aria-expanded", "true");
    consent.hidden = true;
    player.hidden = false;
    player.setAttribute("aria-busy", "true");
    if (status) status.textContent = "Connecting to the privacy-enhanced player…";
    frame.addEventListener("load", function () {
      if (loader.getAttribute("data-youtube-attempt") !== attempt || loader.getAttribute("data-youtube-state") !== "loading" || frame.getAttribute("src") !== source) return;
      loader.setAttribute("data-youtube-state", "ready");
      player.setAttribute("aria-busy", "false");
      if (status) status.textContent = "Privacy-enhanced player ready.";
    }, { once: true });
    frame.addEventListener("error", function () {
      if (loader.getAttribute("data-youtube-attempt") !== attempt || loader.getAttribute("data-youtube-state") !== "loading") return;
      loader.setAttribute("data-youtube-state", "error");
      player.setAttribute("aria-busy", "false");
      if (status) status.textContent = "The player could not load. Open the video on YouTube instead.";
    }, { once: true });
    frame.setAttribute("src", source);
    frame.focus();
    announce("Connecting to the privacy-enhanced YouTube player.", false);
  });

  document.addEventListener("click", function (event) {
    var unloadYouTube = event.target.closest ? event.target.closest("[data-youtube-unload]") : null;
    if (!unloadYouTube) return;
    var loader = unloadYouTube.closest("[data-youtube-loader]");
    var frame = loader ? loader.querySelector("[data-youtube-frame]") : null;
    var consent = loader ? loader.querySelector("[data-youtube-consent]") : null;
    var player = loader ? loader.querySelector("[data-youtube-player]") : null;
    var loadYouTube = loader ? loader.querySelector("[data-youtube-load]") : null;
    var status = loader ? loader.querySelector("[data-youtube-status]") : null;
    if (!loader || !frame || !consent || !player || !loadYouTube) return;
    loader.setAttribute("data-youtube-attempt", String((Number(loader.getAttribute("data-youtube-attempt")) || 0) + 1));
    loader.setAttribute("data-youtube-state", "idle");
    frame.removeAttribute("src");
    player.hidden = true;
    player.setAttribute("aria-busy", "false");
    consent.hidden = false;
    loadYouTube.setAttribute("aria-expanded", "false");
    if (status) status.textContent = "Player unloaded.";
    loadYouTube.focus();
    announce("YouTube player unloaded. It is no longer displayed in this article.", false);
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

  document.addEventListener("click", function (event) {
    var button = event.target.closest ? event.target.closest("[data-copy-share-link]") : null;
    if (!button) return;
    var scope = button.closest(".share-controls-body");
    var input = scope ? scope.querySelector("[data-share-link]") : null;
    if (!input) return;
    input.select();
    input.setSelectionRange(0, input.value.length);
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(input.value)
        .then(function () { announce("Public link copied.", false); })
        .catch(function () { announce("Could not copy the link. Select it manually.", true); });
      return;
    }
    try {
      document.execCommand("copy");
      announce("Public link copied.", false);
    } catch (_) {
      announce("Could not copy the link. Select it manually.", true);
    }
  });

  document.addEventListener("change", function (event) {
    var infinite = event.target.matches ? event.target.matches("[data-share-infinite]") : false;
    if (!infinite) return;
    var form = event.target.closest(".share-form");
    var lifetime = form ? form.querySelector("[data-share-lifetime]") : null;
    if (lifetime) lifetime.disabled = event.target.checked;
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

  function initializeTouchGestures() {
    var root = document.documentElement;
    if (root.getAttribute("data-touch-gestures-enabled") !== "true") return;
    var thresholds = { low: 96, medium: 68, high: 44 };
    var threshold = thresholds[root.getAttribute("data-touch-gesture-sensitivity")] || thresholds.medium;
    var reversed = root.getAttribute("data-touch-gesture-direction") === "reversed";
    var start = null;

    document.addEventListener("touchstart", function (event) {
      if (event.touches.length !== 1 || document.querySelector("dialog[open], .account-menu[open]")) return;
      var target = event.target;
      if (!target.closest || !target.closest(".reading-workspace, [data-current-article], [data-panel-article]")) return;
      if (target.closest("a, button, input, select, textarea, label, summary, iframe, audio, video, [contenteditable], [role='button'], [role='textbox'], [role='combobox']")) return;
      var touch = event.touches[0];
      start = { x: touch.clientX, y: touch.clientY, at: Date.now() };
    }, { passive: true });

    document.addEventListener("touchcancel", function () { start = null; }, { passive: true });
    document.addEventListener("touchend", function (event) {
      if (!start || event.changedTouches.length !== 1) {
        start = null;
        return;
      }
      var touch = event.changedTouches[0];
      var dx = touch.clientX - start.x;
      var dy = touch.clientY - start.y;
      var elapsed = Date.now() - start.at;
      start = null;
      if (elapsed > 900 || Math.abs(dx) < threshold || Math.abs(dx) < Math.abs(dy) * 1.35) return;
      var direction = dx < 0 ? 1 : -1;
      if (reversed) direction *= -1;
      if (openAdjacentArticle(direction)) {
        announce(direction > 0 ? "Next article opened." : "Previous article opened.", false);
      }
    }, { passive: true });
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

  initializeTouchGestures();

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

  var notificationTarget = document.querySelector("[data-notification-count]");
  var notificationTimer = null;
  function refreshNotificationCount() {
    if (!notificationTarget || document.hidden) return;
    fetch(notificationTarget.getAttribute("data-url"), {
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Accept": "text/html" },
    }).then(function (response) {
      if (!response.ok) throw new Error("notification count unavailable");
      return response.text();
    }).then(function (html) {
      notificationTarget.innerHTML = html;
    }).catch(function () {
      // Counts are optional polish; navigation remains fully functional.
    });
  }
  if (notificationTarget) {
    refreshNotificationCount();
    notificationTimer = window.setInterval(refreshNotificationCount, 30000);
    document.addEventListener("visibilitychange", function () {
      if (!document.hidden) refreshNotificationCount();
    });
    window.addEventListener("pagehide", function () {
      if (notificationTimer) window.clearInterval(notificationTimer);
    }, { once: true });
  }

  var cssInput = document.querySelector("[data-css-customization]");
  var cssPreview = document.querySelector("[data-css-preview]");
  if (cssInput && cssPreview) {
    var previewRoot = cssPreview.attachShadow ? cssPreview.attachShadow({ mode: "closed" }) : null;
    var previewSheet = typeof CSSStyleSheet === "function" ? new CSSStyleSheet() : null;
    if (previewRoot) {
      var card = document.createElement("article");
      card.innerHTML = "<strong>Preview card</strong><p>Article text, metadata, and actions appear here.</p><button type='button'>Example action</button>";
      previewRoot.appendChild(card);
      if (previewSheet && "adoptedStyleSheets" in previewRoot) previewRoot.adoptedStyleSheets = [previewSheet];
    }
    var updateCssPreview = function () {
      var css = cssInput.value
        .replace(/@import/gi, "")
        .replace(/url\s*\(/gi, "blocked(");
      if (previewSheet && previewSheet.replaceSync) {
        try { previewSheet.replaceSync(css); } catch (_) { /* Keep the last valid preview. */ }
      }
    };
    cssInput.addEventListener("input", updateCssPreview);
    updateCssPreview();
  }

  function webauthnDecode(value) {
    var base64 = value.replace(/-/g, "+").replace(/_/g, "/");
    while (base64.length % 4) base64 += "=";
    var binary = window.atob(base64);
    var bytes = new Uint8Array(binary.length);
    for (var i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    return bytes.buffer;
  }

  function webauthnEncode(value) {
    var bytes = new Uint8Array(value);
    var binary = "";
    for (var i = 0; i < bytes.length; i += 1) binary += String.fromCharCode(bytes[i]);
    return window.btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }

  function creationOptions(options) {
    var publicKey = options.publicKey;
    publicKey.challenge = webauthnDecode(publicKey.challenge);
    publicKey.user.id = webauthnDecode(publicKey.user.id);
    (publicKey.excludeCredentials || []).forEach(function (credential) {
      credential.id = webauthnDecode(credential.id);
    });
    return publicKey;
  }

  function requestOptions(options) {
    var publicKey = options.publicKey;
    publicKey.challenge = webauthnDecode(publicKey.challenge);
    (publicKey.allowCredentials || []).forEach(function (credential) {
      credential.id = webauthnDecode(credential.id);
    });
    return publicKey;
  }

  function registrationCredential(credential) {
    return {
      id: credential.id,
      rawId: webauthnEncode(credential.rawId),
      type: credential.type,
      response: {
        attestationObject: webauthnEncode(credential.response.attestationObject),
        clientDataJSON: webauthnEncode(credential.response.clientDataJSON),
        transports: credential.response.getTransports ? credential.response.getTransports() : [],
      },
      extensions: credential.getClientExtensionResults(),
    };
  }

  function authenticationCredential(credential) {
    return {
      id: credential.id,
      rawId: webauthnEncode(credential.rawId),
      type: credential.type,
      response: {
        authenticatorData: webauthnEncode(credential.response.authenticatorData),
        clientDataJSON: webauthnEncode(credential.response.clientDataJSON),
        signature: webauthnEncode(credential.response.signature),
        userHandle: credential.response.userHandle ? webauthnEncode(credential.response.userHandle) : null,
      },
      extensions: credential.getClientExtensionResults(),
    };
  }

  function jsonRequest(url, body) {
    return fetch(url, {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "content-type": "application/json", "accept": "application/json" },
      body: JSON.stringify(body),
    }).then(function (response) {
      return response.json().catch(function () { return {}; }).then(function (payload) {
        if (!response.ok) throw new Error(payload.error || payload.message || "The request could not be completed.");
        return payload;
      });
    });
  }

  var passkeyRegistration = document.querySelector("[data-passkey-register]");
  if (passkeyRegistration) {
    passkeyRegistration.addEventListener("submit", function (event) {
      event.preventDefault();
      var status = passkeyRegistration.querySelector("[data-passkey-status]");
      var submit = passkeyRegistration.querySelector("button[type='submit']");
      var values = new FormData(passkeyRegistration);
      submit.disabled = true;
      status.textContent = "Waiting for your authenticator…";
      jsonRequest("/settings/passkeys/register/start", {
        csrf_token: values.get("csrf_token"), name: values.get("name"), password: values.get("password") || "",
      }).then(function (started) {
        return navigator.credentials.create({ publicKey: creationOptions(started.options) }).then(function (credential) {
          if (!credential) throw new Error("No credential was returned.");
          return jsonRequest("/settings/passkeys/register/finish", {
            csrf_token: values.get("csrf_token"), ceremony_id: started.ceremony_id,
            name: values.get("name"), credential: registrationCredential(credential),
          });
        });
      }).then(function () {
        status.textContent = "Passkey added. Reloading…";
        window.location.reload();
      }).catch(function (error) {
        status.textContent = error.name === "NotAllowedError" ? "The authenticator request was cancelled or timed out." : error.message;
        submit.disabled = false;
      });
    });
  }

  var passkeyLogin = document.querySelector("[data-passkey-login]");
  if (passkeyLogin) {
    passkeyLogin.addEventListener("submit", function (event) {
      event.preventDefault();
      var status = passkeyLogin.querySelector("[data-passkey-status]");
      var submit = passkeyLogin.querySelector("button[type='submit']");
      var values = new FormData(passkeyLogin);
      submit.disabled = true;
      status.textContent = "Waiting for your authenticator…";
      jsonRequest("/auth/passkey/start", {
        csrf_token: values.get("csrf_token"), email: values.get("email"),
      }).then(function (started) {
        return navigator.credentials.get({ publicKey: requestOptions(started.options) }).then(function (credential) {
          if (!credential) throw new Error("No credential was returned.");
          return jsonRequest("/auth/passkey/finish", {
            csrf_token: values.get("csrf_token"), ceremony_id: started.ceremony_id,
            credential: authenticationCredential(credential),
          });
        });
      }).then(function (finished) {
        window.location.assign(finished.redirect || "/articles");
      }).catch(function (error) {
        status.textContent = error.name === "NotAllowedError" ? "The authenticator request was cancelled or timed out." : error.message;
        submit.disabled = false;
      });
    });
  }
})();

// Demo-mode banner: shown on every page for ephemeral demo accounts. Driven by
// the readable `rusty_feed_demo` cookie set at /demo/start (CSP-safe, no inline JS).
(function () {
  if (!/(?:^|;\s*)rusty_feed_demo=1(?:;|$)/.test(document.cookie)) return;
  var banner = document.createElement("div");
  banner.className = "demo-banner";
  banner.setAttribute("role", "status");
  banner.textContent =
    "Demo mode — this account and its data reset daily at 04:00 UTC.";
  if (document.body) document.body.insertBefore(banner, document.body.firstChild);
})();

// Theme and first-run wizard live preview (P8 §8.4): swap data-attributes on
// the page / preview pane as options change. CSP-safe (no inline styles/JS).
(function () {
  var themeControls = document.querySelectorAll("input[data-theme-preview-control]");
  themeControls.forEach(function (input) {
    input.addEventListener("change", function () {
      if (!input.checked) return;
      var name = input.getAttribute("data-theme-name") || input.value;
      document.documentElement.setAttribute("data-theme", input.value);
      document.querySelectorAll("[data-theme-preview-name]").forEach(function (target) {
        target.textContent = name;
      });
      var status = document.getElementById("action-status");
      if (status) status.textContent = name + " theme previewed. Save to keep this choice.";
    });
  });

  var controls = document.querySelectorAll("input[data-wizard-attr]");
  var preview = document.getElementById("wizard-preview");
  controls.forEach(function (input) {
    input.addEventListener("change", function () {
      if (!input.checked) return;
      var attr = "data-" + input.getAttribute("data-wizard-attr");
      var target =
        input.getAttribute("data-wizard-target") === "preview"
          ? preview
          : document.documentElement;
      if (target) target.setAttribute(attr, input.value);
    });
  });
})();

// First-run wizard interest filter (P8 §8.4): checking topic chips narrows the
// curated feed suggestions to matching categories. Explains every change with
// a live count + topic names (data-wizard-suggestions-status) instead of just
// letting cards silently appear/disappear. CSP-safe (no inline JS).
(function () {
  var grid = document.querySelector("[data-wizard-suggestions]");
  if (!grid) return;
  var interests = Array.prototype.slice.call(
    document.querySelectorAll("[data-wizard-interest]"),
  );
  if (!interests.length) return;
  var cards = Array.prototype.slice.call(grid.querySelectorAll("[data-wizard-feed]"));
  var empty = grid.querySelector("[data-wizard-suggestions-empty]");
  var status = grid.querySelector("[data-wizard-suggestions-status]");
  var selectionCount = grid.querySelector("[data-wizard-selection-count]");
  function apply() {
    var names = [];
    var selected = Object.create(null);
    interests.forEach(function (input) {
      if (input.checked) {
        selected[input.value] = true;
        names.push(input.value);
      }
    });
    var any = names.length > 0;
    var visible = 0;
    cards.forEach(function (card) {
      var show = !any || selected[card.getAttribute("data-category")] === true;
      card.hidden = !show;
      if (show) visible += 1;
    });
    if (empty) empty.hidden = visible !== 0;
    if (selectionCount) {
      var selectedCount = cards.filter(function (card) {
        var checkbox = card.querySelector("input[name='feed_id']");
        return checkbox && checkbox.checked;
      }).length;
      selectionCount.textContent = selectedCount + " selected.";
    }
    if (status) {
      status.textContent = any
        ? "Showing " + visible + " of " + cards.length + " curated feeds for " + names.join(", ") + "."
        : "Showing all " + cards.length + " curated feeds.";
    }
  }
  interests.forEach(function (input) {
    input.addEventListener("change", apply);
  });
  cards.forEach(function (card) {
    var checkbox = card.querySelector("input[name='feed_id']");
    if (checkbox) checkbox.addEventListener("change", apply);
  });
  apply();
})();

// Dismissible "bring your own AI key" nudge in the article list: hide it once the
// reader dismisses it, remembered locally so it does not nag on every visit.
(function () {
  var cue = document.querySelector("[data-ai-cue]");
  if (!cue) return;
  var KEY = "rf-ai-cue-dismissed";
  try {
    if (window.localStorage && localStorage.getItem(KEY) === "1") {
      cue.hidden = true;
      return;
    }
  } catch (e) {}
  var dismiss = cue.querySelector("[data-ai-cue-dismiss]");
  if (dismiss) {
    dismiss.addEventListener("click", function () {
      cue.hidden = true;
      try {
        if (window.localStorage) localStorage.setItem(KEY, "1");
      } catch (e) {}
    });
  }
})();
