/*
 * Aurora push service worker (C-835, D-267(a), SEC-102).
 *
 * It does exactly two things: show a fixed-text notification when a push arrives, and open an Aurora path when the
 * notification is clicked. It holds no data, serves nothing and makes no network call.
 * Plain JavaScript: no imports, no bundler output.
 */

const TYPES = ["approval.pending", "approval.reminder", "approval.digest", "test"];
const URL_PATTERN = /^\/o\/[A-Za-z0-9._~%/-]+(\?[A-Za-z0-9._~%=&-]*)?$/;

self.addEventListener("install", () => {
  self.skipWaiting();
});

function text(v, max) {
  return typeof v === "string" && v.length > 0 ? v.slice(0, max) : "";
}

function build(raw) {
  let data = null;
  try {
    data = raw ? JSON.parse(raw) : null;
  } catch {
    data = null;
  }
  const ok = data && typeof data === "object" && data.v === 1 && TYPES.indexOf(data.type) !== -1;
  if (!ok) {
    return { title: "Aurora — an approval needs you", body: "Open Aurora to review it.", tag: "aurora-generic", url: "/" };
  }
  const agent = text(data.agent, 40);
  const workspace = text(data.workspace, 40);
  const url = typeof data.url === "string" ? data.url : "/";
  if (data.type === "test") {
    return { title: "Aurora — test notification", body: "Notifications work on this device.", tag: "aurora-test", url: "/" };
  }
  if (data.type === "approval.digest") {
    const n = Number.isInteger(data.count) && data.count > 0 ? data.count : 0;
    return {
      title: n ? "Aurora — " + n + " approvals waiting" : "Aurora — approvals waiting",
      body: "Open Aurora to review them.",
      tag: "aurora-digest",
      url,
    };
  }
  const named = agent && workspace;
  const reminder = data.type === "approval.reminder";
  const title = named
    ? agent + " in " + workspace + " needs an approval"
    : reminder
      ? "Aurora — an approval is still waiting"
      : "Aurora — an approval needs you";
  const body = "Open Aurora to review it." + (data.parked === true ? " The agent has stopped waiting." : "");
  const tag = text(data.approval_id, 64) || "aurora-approval";
  return { title, body, tag, url };
}

self.addEventListener("push", (event) => {
  let raw = "";
  try {
    raw = event.data ? event.data.text() : "";
  } catch {
    raw = "";
  }
  const n = build(raw);
  event.waitUntil(
    self.registration.showNotification(n.title, {
      body: n.body,
      tag: n.tag,
      data: { url: n.url },
      icon: "/icon-192.png",
      requireInteraction: false,
    }),
  );
});

function safeUrl(u) {
  if (typeof u !== "string" || !URL_PATTERN.test(u)) return "/";
  const path = u.split("?")[0];
  if (path.indexOf("//") !== -1 || u.indexOf("\\") !== -1) return "/";
  const parts = path.split("/");
  for (let i = 0; i < parts.length; i++) {
    if (parts[i] === "." || parts[i] === "..") return "/";
  }
  return u;
}

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const data = event.notification.data;
  const url = safeUrl(data && data.url);
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((list) => {
      const mine = list.find((c) => c.url && new URL(c.url).origin === self.location.origin);
      if (mine) {
        return mine.focus().then((c) => (c || mine).navigate(url));
      }
      return self.clients.openWindow(url);
    }),
  );
});

self.addEventListener("pushsubscriptionchange", () => {
  /* nothing: the page re-syncs on its next load */
});
