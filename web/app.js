// @ts-check
// LANShare frontend. Plain JavaScript with JSDoc types so it runs from the
// embedded file with no build step, yet can be type-checked with `tsc`.
//
// Design rules that matter on iOS Safari:
//  - Downloads are plain <a download> links to an attachment endpoint. No
//    blob buffering, so multi-GB files go straight to Files/Downloads.
//  - Previews load the inline endpoint in native <video>/<img> elements;
//    the server supports Range so seeking works.
//  - Uploads use a real <input type=file>, which is what makes iOS offer
//    Photo Library / Files / Camera.

/** @typedef {{id: string, name: string, path: string, root: string, size: number, modified: number|null, mime: string, previewable: boolean, thumbnail: boolean}} FileEntry */
/** @typedef {{version: string, authenticated: boolean, share_name: string, upload_enabled?: boolean, max_upload_bytes?: number, host_name?: string}} Status */

/** Files above this size are not offered via the Share sheet, because the
 *  Web Share API needs the whole file in memory first. */
const SHARE_SIZE_LIMIT = 200 * 1024 * 1024;
/** Text previews are fetched into the page, so cap them. */
const TEXT_PREVIEW_LIMIT = 1024 * 1024;

const $ = (/** @type {string} */ id) => /** @type {HTMLElement} */ (document.getElementById(id));

const views = {
  login: $("view-login"),
  files: $("view-files"),
};

/** @type {Status | null} */
let status = null;

// ---------------------------------------------------------------- API ---

/**
 * fetch() with the conventions of our API: same-origin cookies, JSON errors,
 * and a 401 anywhere after login means the session is gone.
 * @param {string} path
 * @param {RequestInit} [init]
 */
async function api(path, init = {}) {
  const res = await fetch(path, { credentials: "same-origin", ...init });
  if (res.status === 401 && status && status.authenticated) {
    status.authenticated = false;
    showLogin("Session ended. Enter the code again.");
    throw new Error("session expired");
  }
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try {
      const body = await res.json();
      if (body && body.error) message = body.error;
    } catch (_) { /* non-JSON error body: keep the status text */ }
    throw new Error(message);
  }
  return res;
}

// -------------------------------------------------------------- views ---

/** @param {string} [message] */
function showLogin(message) {
  views.files.hidden = true;
  views.login.hidden = false;
  $("topbar").hidden = true;
  const err = $("login-error");
  err.hidden = !message;
  err.textContent = message || "";
  const code = /** @type {HTMLInputElement} */ ($("code"));
  code.value = "";
  code.focus();
}

function showFiles() {
  views.login.hidden = true;
  views.files.hidden = false;
  $("topbar").hidden = false;
  $("upload-panel").hidden = !(status && status.upload_enabled);
  loadFiles();
}

async function init() {
  try {
    const res = await fetch("/api/status", { credentials: "same-origin" });
    status = await res.json();
  } catch (e) {
    showLogin("Cannot reach the server.");
    return;
  }
  if (status) {
    const name = status.share_name || "LANShare";
    $("share-name").textContent = name;
    $("splash-name").textContent = name;
    $("splash-host").textContent = status.host_name ? `shared from ${status.host_name}` : "";
    document.title = `${name} – LANShare`;
  }
  if (status && status.authenticated) showFiles();
  else showLogin();
}

// -------------------------------------------------------------- login ---

/**
 * A short label for the activity feed on the sharing computer. iPadOS
 * Safari sends a Mac User-Agent, so the server cannot tell; touch support
 * is the giveaway, and that is only visible from here.
 */
function deviceName() {
  const ua = navigator.userAgent;
  const touch = navigator.maxTouchPoints > 1;
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPad/.test(ua) || (/Macintosh/.test(ua) && touch)) return "iPad";
  if (/Android/.test(ua)) return /Mobile/.test(ua) ? "Android phone" : "Android tablet";
  if (/Macintosh/.test(ua)) return "Mac";
  if (/Windows/.test(ua)) return "Windows PC";
  if (/CrOS/.test(ua)) return "Chromebook";
  if (/Linux/.test(ua)) return "Linux PC";
  return "Browser";
}

$("login-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = /** @type {HTMLFormElement} */ (event.currentTarget);
  const button = /** @type {HTMLButtonElement} */ (form.querySelector("button"));
  const code = /** @type {HTMLInputElement} */ ($("code")).value;
  button.disabled = true;
  try {
    await api("/api/auth", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, device: deviceName() }),
    });
    if (status) status.authenticated = true;
    showFiles();
  } catch (e) {
    const err = $("login-error");
    err.textContent = e instanceof Error ? e.message : "Login failed";
    err.hidden = false;
  } finally {
    button.disabled = false;
  }
});

$("logout").addEventListener("click", async () => {
  try { await api("/api/logout", { method: "POST" }); } catch (_) { /* already gone */ }
  if (status) status.authenticated = false;
  showLogin();
});

// -------------------------------------------------------------- files ---

/** @type {FileEntry[]} */
let files = [];

/** @type {"list" | "grid"} */
let view = "list";
let viewChosen = false;
try {
  const saved = localStorage.getItem("lanshare-view");
  if (saved === "list" || saved === "grid") { view = saved; viewChosen = true; }
} catch (_) { /* private mode or storage disabled: default stays */ }

/**
 * Thumbnail URL. The query carries size and mtime so a changed file gets a
 * new URL and the browser's day-long cache never shows a stale image.
 * @param {FileEntry} file
 * @param {"small" | "large"} size
 */
function thumbUrl(file, size) {
  return `/api/files/${encodeURIComponent(file.id)}/thumb?size=${size}&v=${file.size}-${file.modified || 0}`;
}

/**
 * Fill a `.file-thumb` box with either a lazy-loaded thumbnail or a type
 * badge. A thumbnail that fails to load (corrupt file, decoder error)
 * degrades to the badge rather than a broken-image icon.
 * @param {HTMLElement} box
 * @param {FileEntry} file
 */
function fillThumb(box, file) {
  const badge = () => { box.textContent = badgeText(file); };
  if (!file.thumbnail) { badge(); return; }
  const img = document.createElement("img");
  img.loading = "lazy";
  img.decoding = "async";
  img.alt = "";
  img.src = thumbUrl(file, "small");
  img.addEventListener("error", () => {
    badge();
    box.closest(".cell")?.classList.remove("has-thumb");
  });
  box.textContent = "";
  box.appendChild(img);
}

/** @param {FileEntry} file */
function badgeText(file) {
  const ext = file.name.includes(".") ? file.name.slice(file.name.lastIndexOf(".") + 1) : "";
  if (ext && ext.length <= 5) return ext;
  const [kind] = file.mime.split("/");
  return kind === "application" ? "file" : kind;
}

/** @param {"list" | "grid"} next */
function setView(next) {
  view = next;
  $("file-list").hidden = view !== "list";
  $("file-grid").hidden = view !== "grid";
  $("view-list").setAttribute("aria-pressed", String(view === "list"));
  $("view-grid").setAttribute("aria-pressed", String(view === "grid"));
}

$("view-list").addEventListener("click", () => { viewChosen = true; remember("list"); setView("list"); });
$("view-grid").addEventListener("click", () => { viewChosen = true; remember("grid"); setView("grid"); });

/** @param {"list" | "grid"} v */
function remember(v) {
  try { localStorage.setItem("lanshare-view", v); } catch (_) { /* fine without */ }
}

async function loadFiles() {
  const error = $("files-error");
  error.hidden = true;
  // Only show the spinner on first load; a refresh keeps the old list in
  // place so the page does not flash.
  $("loading").hidden = files.length > 0;
  try {
    const res = await api("/api/files");
    const body = await res.json();
    files = body.files;
    renderFiles(body.truncated);
  } catch (e) {
    if (e instanceof Error && e.message === "session expired") return;
    error.textContent = e instanceof Error ? e.message : "Could not load files";
    error.hidden = false;
  } finally {
    $("loading").hidden = true;
  }
}

/** @param {boolean} truncated */
function renderFiles(truncated) {
  const list = $("file-list");
  const grid = $("file-grid");
  const template = /** @type {HTMLTemplateElement} */ ($("file-item"));
  const cellTemplate = /** @type {HTMLTemplateElement} */ ($("grid-item"));
  list.textContent = "";
  grid.textContent = "";
  $("empty").hidden = files.length > 0;

  // A folder of photos reads better as a grid; anything else as a list.
  // Only applied until the person picks a view themselves.
  if (!viewChosen) {
    const withThumbs = files.filter((f) => f.thumbnail).length;
    setView(files.length > 0 && withThumbs * 2 >= files.length ? "grid" : "list");
  } else {
    setView(view);
  }
  $("file-count").textContent = files.length === 1
    ? "1 file"
    : `${files.length} files${truncated ? " (list truncated)" : ""}`;

  const multiRoot = new Set(files.map((f) => f.root)).size > 1;
  const canShare = typeof navigator.canShare === "function";

  for (const file of files) {
    const node = /** @type {HTMLElement} */ (template.content.firstElementChild?.cloneNode(true));
    /** @type {HTMLElement} */ (node.querySelector(".file-name")).textContent = file.name;
    fillThumb(/** @type {HTMLElement} */ (node.querySelector(".file-thumb")), file);

    const meta = [formatSize(file.size), shortType(file.mime)];
    const folder = multiRoot ? `${file.root}/${file.path}` : file.path;
    if (folder !== file.name) meta.push(folder.slice(0, folder.length - file.name.length - 1) || file.root);
    if (file.modified) meta.push(new Date(file.modified * 1000).toLocaleDateString());
    /** @type {HTMLElement} */ (node.querySelector(".file-meta")).textContent = meta.join(" · ");

    const download = /** @type {HTMLAnchorElement} */ (node.querySelector(".act-download"));
    download.href = `/api/files/${encodeURIComponent(file.id)}/download`;
    download.download = file.name;

    const preview = /** @type {HTMLButtonElement} */ (node.querySelector(".act-preview"));
    preview.hidden = !file.previewable;
    preview.addEventListener("click", () => openPreview(file));

    const share = /** @type {HTMLButtonElement} */ (node.querySelector(".act-share"));
    share.hidden = !(canShare && file.size <= SHARE_SIZE_LIMIT);
    share.addEventListener("click", () => shareFile(file, share));

    list.appendChild(node);

    const cell = /** @type {HTMLElement} */ (cellTemplate.content.firstElementChild?.cloneNode(true));
    fillThumb(/** @type {HTMLElement} */ (cell.querySelector(".cell-thumb")), file);
    cell.classList.toggle("has-thumb", file.thumbnail);
    /** @type {HTMLElement} */ (cell.querySelector(".cell-name")).textContent = file.name;
    cell.querySelector(".cell-button")?.addEventListener("click", () => {
      if (file.previewable) openPreview(file);
      else download.click();
    });
    grid.appendChild(cell);
  }
}

$("refresh").addEventListener("click", loadFiles);

/** @param {number} bytes */
function formatSize(bytes) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${unit === 0 ? value : value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

/** @param {string} mime */
function shortType(mime) {
  const [kind, sub] = mime.split("/");
  if (kind === "image" || kind === "video" || kind === "audio") return `${kind} (${sub})`;
  if (mime === "application/pdf") return "PDF";
  if (mime === "application/zip") return "ZIP";
  if (mime === "application/octet-stream") return "file";
  return sub || mime;
}

// ------------------------------------------------------------ preview ---

/** @param {FileEntry} file */
async function openPreview(file) {
  const url = `/api/files/${encodeURIComponent(file.id)}/preview`;
  const body = $("preview-body");
  body.textContent = "";
  $("preview-name").textContent = file.name;
  const dl = /** @type {HTMLAnchorElement} */ ($("preview-download"));
  dl.href = `/api/files/${encodeURIComponent(file.id)}/download`;
  dl.download = file.name;
  $("preview").hidden = false;

  if (file.mime.startsWith("image/")) {
    const img = document.createElement("img");
    img.src = url;
    img.alt = file.name;
    // Safari shows HEIC natively; Chrome and Firefox cannot. When the
    // original fails to decode, fall back to a server-rendered JPEG. The
    // Download button still delivers the untouched original.
    if (file.thumbnail) {
      img.addEventListener("error", () => {
        if (img.dataset.fallback) return;
        img.dataset.fallback = "1";
        img.src = thumbUrl(file, "large");
        const note = document.createElement("p");
        note.className = "hint overlay-note";
        note.textContent = "Converted preview; Download gives the original file.";
        body.appendChild(note);
      }, { once: false });
    }
    body.appendChild(img);
  } else if (file.mime.startsWith("video/")) {
    const video = document.createElement("video");
    video.src = url;
    video.controls = true;
    video.autoplay = true;
    // playsinline: without it iPhone Safari takes over the whole screen.
    video.setAttribute("playsinline", "");
    body.appendChild(video);
  } else if (file.mime.startsWith("audio/")) {
    const audio = document.createElement("audio");
    audio.src = url;
    audio.controls = true;
    audio.autoplay = true;
    body.appendChild(audio);
  } else if (file.mime === "application/pdf") {
    const frame = document.createElement("iframe");
    frame.src = url;
    frame.title = file.name;
    body.appendChild(frame);
  } else {
    const pre = document.createElement("pre");
    pre.textContent = "Loading…";
    body.appendChild(pre);
    try {
      const res = await api(url, { headers: { Range: `bytes=0-${TEXT_PREVIEW_LIMIT - 1}` } });
      pre.textContent = await res.text();
      if (file.size > TEXT_PREVIEW_LIMIT) pre.textContent += "\n\n[preview truncated]";
    } catch (e) {
      pre.textContent = e instanceof Error ? e.message : "Preview failed";
    }
  }
}

function closePreview() {
  $("preview").hidden = true;
  // Dropping the elements stops playback and releases the stream.
  $("preview-body").textContent = "";
}

$("preview-close").addEventListener("click", closePreview);
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closePreview(); });

// -------------------------------------------------------------- share ---

/**
 * iOS share sheet, which is the only browser route into the Photos app.
 * Needs the bytes in memory, hence the size cap, and usually a secure
 * context, which is why the button is hidden when `canShare` is missing.
 * @param {FileEntry} file
 * @param {HTMLButtonElement} button
 */
async function shareFile(file, button) {
  button.disabled = true;
  const label = button.textContent;
  button.textContent = "Preparing…";
  try {
    const res = await api(`/api/files/${encodeURIComponent(file.id)}/download`);
    const blob = await res.blob();
    const shared = new File([blob], file.name, { type: file.mime });
    if (!navigator.canShare({ files: [shared] })) {
      throw new Error("This file type cannot be shared from the browser.");
    }
    await navigator.share({ files: [shared], title: file.name });
  } catch (e) {
    // AbortError is the user dismissing the sheet: not an error worth showing.
    if (!(e instanceof DOMException && e.name === "AbortError")) {
      alert(e instanceof Error ? e.message : "Share failed");
    }
  } finally {
    button.disabled = false;
    button.textContent = label;
  }
}

// ------------------------------------------------------------- upload ---

/** @type {XMLHttpRequest | null} */
let activeUpload = null;

$("upload-input").addEventListener("change", (event) => {
  const input = /** @type {HTMLInputElement} */ (event.currentTarget);
  if (input.files && input.files.length > 0) uploadFiles(Array.from(input.files));
  // Reset so picking the same file again re-triggers `change`.
  input.value = "";
});

/** @param {File[]} chosen */
function uploadFiles(chosen) {
  const result = $("upload-result");
  result.hidden = true;

  const limit = status && status.max_upload_bytes;
  const total = chosen.reduce((sum, f) => sum + f.size, 0);
  if (limit && total > limit) {
    result.textContent = `Too large: ${formatSize(total)} exceeds the ${formatSize(limit)} limit.`;
    result.hidden = false;
    return;
  }

  // XMLHttpRequest rather than fetch(): it is still the only way to get
  // upload progress events in Safari.
  const xhr = new XMLHttpRequest();
  activeUpload = xhr;
  const form = new FormData();
  for (const f of chosen) form.append("file", f, f.name);

  const bar = $("upload-bar");
  const text = $("upload-text");
  $("upload-status").hidden = false;
  bar.style.width = "0%";
  text.textContent = `Uploading ${chosen.length === 1 ? chosen[0].name : chosen.length + " files"}…`;

  xhr.upload.addEventListener("progress", (e) => {
    if (!e.lengthComputable) return;
    const pct = Math.round((e.loaded / e.total) * 100);
    bar.style.width = `${pct}%`;
    text.textContent = `${pct}% · ${formatSize(e.loaded)} of ${formatSize(e.total)}`;
  });

  const finish = (/** @type {string} */ message) => {
    activeUpload = null;
    $("upload-status").hidden = true;
    result.textContent = message;
    result.hidden = false;
  };

  xhr.addEventListener("load", () => {
    let body = null;
    try { body = JSON.parse(xhr.responseText); } catch (_) { /* fall through */ }
    if (xhr.status >= 200 && xhr.status < 300 && body) {
      const saved = body.saved.map((/** @type {{name: string}} */ s) => s.name).join(", ");
      const failed = body.failed.length ? ` Failed: ${body.failed.map((/** @type {{name: string, error: string}} */ f) => `${f.name} (${f.error})`).join(", ")}` : "";
      finish(`Saved: ${saved || "nothing"}.${failed}`);
      loadFiles();
    } else if (xhr.status === 401) {
      if (status) status.authenticated = false;
      showLogin("Session ended. Enter the code again.");
    } else {
      finish(`Upload failed: ${(body && body.error) || xhr.statusText || "unknown error"}`);
    }
  });
  xhr.addEventListener("error", () => finish("Upload failed: connection lost."));
  xhr.addEventListener("abort", () => finish("Upload cancelled."));

  xhr.open("POST", "/api/upload");
  xhr.send(form);
}

$("upload-cancel").addEventListener("click", () => { if (activeUpload) activeUpload.abort(); });

// ---------------------------------------------------------------- go! ---

init();
