# 1.2.15 — Interlude (connection & stability fixes)

I've been a bit heads-down lately — I'm building a major update to qbz. What I can tell you so far: it doesn't change what the app is at heart, it's an upgrade into a more mature product, and I'm more excited about it the further it gets. I'm hoping to have it ready within a couple of weeks.

This release isn't that one. It's a small maintenance build with no new features — just two bug fixes: one for when the connection to Qobuz falters (and left the app stuck before the login screen), and one that could close the app with a "Too many open files" error.

---

## Connection & startup

  - **No more frozen startup** — when Qobuz is slow to serve its web bundle, qbz used to sit on a blank screen for half a minute before the login screen ever appeared. The download is now bounded by a timeout and retried instead of hanging.
  - **Cached connection tokens** — the tokens qbz needs from Qobuz are cached on disk after the first launch, so later starts go straight to the app instead of re-downloading them every time.
  - **Connecting feedback** — the first launch shows a "connecting to Qobuz" state instead of an unresponsive splash.

---

## Stability

  - **"Too many open files" crash fixed** — image-heavy views like Discover could exhaust the system's open-file limit and close the app; image and artwork downloads now reuse a single shared connection instead of opening a new one per image.

---

Full changelog: https://github.com/vicrodh/qbz/compare/v1.2.14...v1.2.15
