// SDM browser integration — hands download links to the SDM app via its
// native-messaging host (com.sdm.catch). Original code; standard WebExtension APIs.

const HOST = "com.sdm.catch";

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "sdm-link",
    title: "Download with SDM",
    contexts: ["link", "image", "video", "audio"],
  });
  chrome.contextMenus.create({
    id: "sdm-page",
    title: "Download this page with SDM",
    contexts: ["page", "frame", "selection"],
  });
  refreshBadge();
});
chrome.runtime.onStartup.addListener(refreshBadge);

// Explicit, always-on: right-click → Download with SDM.
chrome.contextMenus.onClicked.addListener((info, tab) => {
  const url = info.linkUrl || info.srcUrl || info.pageUrl;
  if (url) catchUrl(url, info.pageUrl || (tab && tab.url) || "");
});

// Auto-catch: hand browser-initiated downloads to SDM (toggle with the icon).
chrome.downloads.onCreated.addListener(async (item) => {
  const { intercept = true } = await chrome.storage.local.get("intercept");
  if (!intercept) return;
  const url = item.finalUrl || item.url;
  if (!url || !/^https?:\/\//i.test(url)) return;
  chrome.downloads.cancel(item.id, () => {
    chrome.downloads.erase({ id: item.id }, () => void chrome.runtime.lastError);
  });
  catchUrl(url, item.referrer || "", filenameOf(item.filename));
});

// Click the toolbar icon to pause / resume auto-catching.
chrome.action.onClicked.addListener(async () => {
  const { intercept = true } = await chrome.storage.local.get("intercept");
  await chrome.storage.local.set({ intercept: !intercept });
  refreshBadge();
});

function filenameOf(p) {
  if (!p) return "";
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] || "";
}

async function catchUrl(url, referrer, filename) {
  let cookies = "";
  try {
    const all = await chrome.cookies.getAll({ url });
    cookies = all.map((c) => c.name + "=" + c.value).join("; ");
  } catch (e) {
    /* no cookie access for this URL */
  }
  const message = {
    url,
    referrer: referrer || "",
    cookies,
    userAgent: navigator.userAgent,
    filename: filename || "",
  };
  chrome.runtime.sendNativeMessage(HOST, message, (resp) => {
    if (chrome.runtime.lastError) {
      notify("SDM not reachable", "Couldn't reach SDM. Is it installed and registered?");
    } else if (!resp || !resp.ok) {
      notify("SDM", "SDM did not accept the download.");
    }
  });
}

async function refreshBadge() {
  const { intercept = true } = await chrome.storage.local.get("intercept");
  try {
    chrome.action.setBadgeText({ text: intercept ? "" : "off" });
    chrome.action.setBadgeBackgroundColor({ color: "#9a6700" });
    chrome.action.setTitle({
      title: intercept
        ? "SDM: catching downloads (click to pause)"
        : "SDM: download catching paused (click to resume)",
    });
  } catch (e) {
    /* action API unavailable */
  }
}

function notify(title, message) {
  try {
    chrome.notifications.create({ type: "basic", iconUrl: "icon48.png", title, message });
  } catch (e) {
    /* notifications unavailable */
  }
}
