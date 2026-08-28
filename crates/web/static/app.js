// RustSS progressive enhancement (M5). The UI works fully without JS; this just
// makes actions in-place and enables inline reading. Vanilla JS, no dependencies.
(function () {
  "use strict";

  // In-place article actions (mark read/unread, save/unsave, mark all read).
  document.addEventListener("submit", function (e) {
    var form = e.target;
    if (!form.classList || !form.classList.contains("js-action")) return;
    e.preventDefault();

    fetch(form.action, {
      method: "POST",
      body: new FormData(form),
      headers: { "X-Requested-With": "fetch" },
      credentials: "same-origin",
    })
      .then(function (r) {
        if (!r.ok) throw new Error("action failed");
        var targetSel = form.getAttribute("data-target");
        var toggle = form.getAttribute("data-toggle-class");
        if (targetSel && toggle && toggle !== "noop") {
          var target = document.querySelector(targetSel);
          if (target) target.classList.toggle(toggle);
        }
        // Swap the form's action + button label to its opposite state.
        var alt = form.getAttribute("data-alt-action");
        if (alt) {
          form.setAttribute("data-alt-action", form.action);
          form.action = alt;
        }
        var altLabel = form.getAttribute("data-alt-label");
        var btn = form.querySelector("button");
        if (btn && altLabel) {
          form.setAttribute("data-alt-label", btn.textContent.trim());
          btn.textContent = altLabel;
        }
      })
      .catch(function () {
        form.removeEventListener("submit", arguments.callee);
        form.submit(); // fall back to a normal submit
      });
  });

  // Inline reading: expand article content in the list instead of navigating.
  document.addEventListener("click", function (e) {
    var link = e.target.closest ? e.target.closest("a.article-link") : null;
    if (!link) return;
    var list = link.closest(".article-list");
    if (!list || list.getAttribute("data-reading-mode") !== "inline") return;

    e.preventDefault();
    var card = link.closest(".article-card");
    var panel = card ? card.querySelector(".article-inline") : null;
    if (!panel) {
      window.location = link.href;
      return;
    }
    if (!panel.hidden) {
      panel.hidden = true;
      return;
    }
    if (panel.getAttribute("data-loaded") === "1") {
      panel.hidden = false;
      return;
    }
    fetch(link.href + "/content", {
      headers: { "X-Requested-With": "fetch" },
      credentials: "same-origin",
    })
      .then(function (r) {
        return r.ok ? r.text() : Promise.reject();
      })
      .then(function (html) {
        panel.innerHTML = html;
        panel.setAttribute("data-loaded", "1");
        panel.hidden = false;
        card.classList.add("read");
      })
      .catch(function () {
        window.location = link.href;
      });
  });
})();
