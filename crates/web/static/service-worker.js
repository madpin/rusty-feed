// RustSS caches only versioned public shell assets. Authenticated pages,
// articles, API responses, and publisher content always stay network-only.
var CACHE_NAME = "rustss-static-v1";
var STATIC_ASSETS = [
  "/static/app.css",
  "/static/app.js",
  "/static/icon.svg",
  "/static/icon-192.png",
  "/static/icon-512.png",
];

self.addEventListener("install", function (event) {
  event.waitUntil(
    caches.open(CACHE_NAME).then(function (cache) {
      return Promise.all(STATIC_ASSETS.map(function (path) {
        return fetch(path, { credentials: "omit", cache: "reload" }).then(function (response) {
          if (!response.ok) throw new Error("Could not cache public RustSS asset");
          return cache.put(path, response);
        });
      }));
    }).then(function () {
      return self.skipWaiting();
    })
  );
});

self.addEventListener("activate", function (event) {
  event.waitUntil(
    caches.keys().then(function (keys) {
      return Promise.all(keys.filter(function (key) {
        return key.indexOf("rustss-static-") === 0 && key !== CACHE_NAME;
      }).map(function (key) {
        return caches.delete(key);
      }));
    }).then(function () {
      return self.clients.claim();
    })
  );
});

self.addEventListener("fetch", function (event) {
  if (event.request.method !== "GET") return;
  var url = new URL(event.request.url);
  if (url.origin !== self.location.origin || STATIC_ASSETS.indexOf(url.pathname) === -1) return;
  event.respondWith(
    caches.open(CACHE_NAME).then(function (cache) {
      return cache.match(url.pathname).then(function (cached) {
        var update = fetch(url.pathname, { credentials: "omit", cache: "no-cache" }).then(function (response) {
          if (!response.ok) return response;
          return cache.put(url.pathname, response.clone()).then(function () { return response; });
        }).catch(function () {
          return cached || Response.error();
        });
        return cached || update;
      });
    })
  );
});
