/* Tijdsregistratie service worker. Bump CACHE_VERSION to invalidate all caches. */
const CACHE_VERSION = "v2";
const CACHE = `tijdsregistratie-${CACHE_VERSION}`;
const SHELL = [
  "/",
  "/manifest.webmanifest",
  "/icon.svg",
  "/icon-192.png",
  "/icon-512.png",
  "/icon-maskable-512.png",
  "/apple-touch-icon.png",
  "/favicon.ico",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((key) => key.startsWith("tijdsregistratie-") && key !== CACHE)
            .map((key) => caches.delete(key)),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

function isStaticAsset(url) {
  return (
    url.pathname.startsWith("/assets/") ||
    url.pathname.startsWith("/wasm/") ||
    /\.(?:wasm|js|css|png|svg|ico|webp|woff2?|webmanifest)$/.test(url.pathname)
  );
}

async function networkFirstNavigation(request) {
  const cache = await caches.open(CACHE);
  try {
    const response = await fetch(request);
    if (response.ok && new URL(request.url).pathname === "/") {
      cache.put("/", response.clone());
    }
    return response;
  } catch (err) {
    const cached = await cache.match("/");
    if (cached) return cached;
    throw err;
  }
}

async function cacheFirstWithRefresh(event) {
  const cache = await caches.open(CACHE);
  const cached = await cache.match(event.request);
  const refresh = fetch(event.request)
    .then((response) => {
      if (response.ok && response.type === "basic") {
        cache.put(event.request, response.clone());
      }
      return response;
    })
    .catch(() => undefined);
  if (cached) {
    event.waitUntil(refresh);
    return cached;
  }
  const response = await refresh;
  return response || Response.error();
}

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;
  // Never cache API / server function traffic, the service worker itself or the generated theme
  // (it changes with the organisation's brand colour).
  if (url.pathname.startsWith("/api/") || url.pathname === "/sw.js" || url.pathname === "/theme.css") return;

  if (request.mode === "navigate") {
    event.respondWith(networkFirstNavigation(request));
  } else if (isStaticAsset(url)) {
    event.respondWith(cacheFirstWithRefresh(event));
  }
});
