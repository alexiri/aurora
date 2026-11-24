/*
 * Aurora push service worker (C-835, D-267(a), SEC-102).
 *
 * It does exactly two things: show a fixed-text notification when a push arrives, and open an Aurora path when the
 * notification is clicked. It holds no data, serves nothing and makes no network call.
 * Plain JavaScript: no imports, no bundler output.
 */

const TYPES = [
  "approval.pending",
  "approval.reminder",
  "approval.digest",
  "test",
  "admin_attention",
  "app_setup_request",
  "app_setup_resolved",
  "app_setup_declined",
  "schedule_answer",
  "self_grant",
];
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
  if (data.type === "admin_attention") {
    // (FIX-167, C-1023) Fixed text; the item's title only when the device turned names on.
    const item = text(data.title, 80);
    return {
      title: item ? "Aurora — " + item : "Aurora — something in Administration needs your decision",
      body: "Open Administration to review it.",
      tag: "aurora-admin",
      url: "/admin",
    };
  }
  if (data.type === "self_grant") {
    // (FIX-234, C-1161) Fixed text; the sentence only when the device turned names on. Never the
    // workspace's content.
    const item = text(data.title, 80);
    return {
      title: item ? "Aurora — " + item : "Aurora — someone gave themselves workspace access",
      body: "Open the workspace's People page to review it.",
      tag: "aurora-self-grant",
      url: URL_PATTERN.test(url) ? url : "/",
    };
  }
  if (data.type === "app_setup_request" || data.type === "app_setup_resolved" || data.type === "app_setup_declined") {
    // (P5-POLISH-4, D-386) Fixed text per type; the notice's title only when the device turned names on.
    const item = text(data.title, 80);
    const fixed = {
      app_setup_request: ["Aurora — someone asked for an app", "Open Aurora to set it up or decline."],
      app_setup_resolved: ["Aurora — an app you asked for is ready", "Give it to your agent from its Access tab."],
      app_setup_declined: ["Aurora — an app you asked for won't be set up now", "Open Aurora for details."],
    }[data.type];
    return {
      title: item ? "Aurora — " + item : fixed[0],
      body: fixed[1],
      tag: "aurora-app-setup",
      url: URL_PATTERN.test(url) ? url : "/",
    };
  }
  if (data.type === "schedule_answer") {
    // (P5-SCHED-ONCE, C-835) Fixed text, never the answer; the schedule's name only when the device turned names on. The
    // click opens exactly the conversation path the push carried.
    const item = text(data.title, 80);
    return {
      title: item ? "Aurora — " + item : "Aurora — a scheduled task has an answer for you",
      body: "Open the conversation to read it.",
      tag: "aurora-schedule",
      url: URL_PATTERN.test(url) ? url : "/",
    };
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
  if (u === "/admin") return u;
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
        // A refused focus still navigates; a tab this worker does not control (opened before the opt-in) refuses
        // navigate(), and a new window is opened instead.
        return mine
          .focus()
          .catch(() => mine)
          .then((c) => (c || mine).navigate(url))
          .catch(() => self.clients.openWindow(url));
      }
      return self.clients.openWindow(url);
    }),
  );
});

self.addEventListener("pushsubscriptionchange", () => {
  /* nothing: the page re-syncs on its next load */
});
