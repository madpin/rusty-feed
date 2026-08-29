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

  function initializePanelResize() {
    var workspace = document.querySelector(".reading-workspace");
    var handle = workspace ? workspace.querySelector(".panel-resizer") : null;
    if (!workspace || !handle) return;
    function setWidth(value) {
      var width = Math.round(Math.max(38, Math.min(68, value)));
      workspace.style.setProperty("--reader-panel-width", width + "%");
      handle.setAttribute("aria-valuenow", String(width));
    }
    function widthFromPointer(clientX) {
      var rect = workspace.getBoundingClientRect();
      return ((rect.right - clientX) / rect.width) * 100;
    }
    handle.addEventListener("pointerdown", function (event) {
      if (!window.matchMedia("(min-width: 54.0625rem)").matches) return;
      handle.dataset.resizing = "true";
      handle.setPointerCapture(event.pointerId);
      setWidth(widthFromPointer(event.clientX));
      event.preventDefault();
    });
    handle.addEventListener("pointermove", function (event) {
      if (handle.dataset.resizing === "true") setWidth(widthFromPointer(event.clientX));
    });
    handle.addEventListener("pointerup", function (event) {
      handle.dataset.resizing = "false";
      if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
    });
    handle.addEventListener("keydown", function (event) {
      var current = Number(handle.getAttribute("aria-valuenow")) || 56;
      if (event.key === "ArrowLeft") setWidth(current + 2);
      else if (event.key === "ArrowRight") setWidth(current - 2);
      else if (event.key === "Home") setWidth(38);
      else if (event.key === "End") setWidth(68);
      else return;
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

  // Implicit reading signal: on an article page, report time spent when the
  // reader leaves, so personalization can learn from completion/bounce.
  initializeReadingTracking(document);
  initializePanelResize();

  // Inline reading: expand article content in the list instead of navigating.
  document.addEventListener("click", function (event) {
    var link = event.target.closest ? event.target.closest("a.article-link") : null;
    if (!link) return;
    var list = link.closest(".article-list");
    if (!list) return;
    var mode = list.getAttribute("data-reading-mode");
    if (mode !== "inline" && mode !== "side-panel") return;
    if (mode === "side-panel" && !window.matchMedia("(min-width: 54.0625rem)").matches) return;

    event.preventDefault();
    var panel;
    if (mode === "inline") {
      panel = link.closest(".article-card").querySelector(".article-inline");
      if (!panel.hidden && panel.getAttribute("data-panel-url") === link.href + "/panel") {
        reportReadingWithin(panel);
        panel.hidden = true;
        link.setAttribute("aria-expanded", "false");
        return;
      }
      Array.prototype.forEach.call(list.querySelectorAll(".article-inline"), function (other) {
        if (other !== panel) {
          reportReadingWithin(other);
          other.hidden = true;
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
})();
