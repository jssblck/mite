const $ = (id) => document.getElementById(id);

const els = {
  rootLabel: $("rootLabel"),
  reloadBtn: $("reloadBtn"),
  uploadInput: $("uploadInput"),
  runOcrBtn: $("runOcrBtn"),
  fitBtn: $("fitBtn"),
  showLabels: $("showLabels"),
  showRaw: $("showRaw"),
  showDiff: $("showDiff"),
  showChars: $("showChars"),
  showIgnored: $("showIgnored"),
  imageStats: $("imageStats"),
  searchBox: $("searchBox"),
  imageList: $("imageList"),
  selectedPath: $("selectedPath"),
  statusLine: $("statusLine"),
  selectModeBtn: $("selectModeBtn"),
  drawLabelBtn: $("drawLabelBtn"),
  drawIgnoreBtn: $("drawIgnoreBtn"),
  canvasWrap: $("canvasWrap"),
  canvas: $("imageCanvas"),
  emptyState: $("emptyState"),
  tabLabels: $("tabLabels"),
  tabRaw: $("tabRaw"),
  tabDiff: $("tabDiff"),
  tabJson: $("tabJson"),
  labelsPanel: $("labelsPanel"),
  rawPanel: $("rawPanel"),
  diffPanel: $("diffPanel"),
  jsonPanel: $("jsonPanel"),
  addLabelBtn: $("addLabelBtn"),
  addIgnoreBtn: $("addIgnoreBtn"),
  undoBtn: $("undoBtn"),
  redoBtn: $("redoBtn"),
  saveBtn: $("saveBtn"),
  labelSummary: $("labelSummary"),
  labelList: $("labelList"),
  labelForm: $("labelForm"),
  selKind: $("selKind"),
  detId: $("detId"),
  detText: $("detText"),
  ignoreReason: $("ignoreReason"),
  rectX: $("rectX"),
  rectY: $("rectY"),
  rectW: $("rectW"),
  rectH: $("rectH"),
  tolX: $("tolX"),
  tolY: $("tolY"),
  tolW: $("tolW"),
  tolH: $("tolH"),
  detNotes: $("detNotes"),
  applyFormBtn: $("applyFormBtn"),
  rebuildBtn: $("rebuildBtn"),
  deleteBtn: $("deleteBtn"),
  rawSummary: $("rawSummary"),
  rawList: $("rawList"),
  diffSummary: $("diffSummary"),
  diffList: $("diffList"),
  jsonEditor: $("jsonEditor"),
  formatJsonBtn: $("formatJsonBtn"),
  saveJsonBtn: $("saveJsonBtn"),
  diagState: $("diagState"),
};

const ctx = els.canvas.getContext("2d");
const image = new Image();
image.decoding = "async";

const UI_STATE_KEY = "miteEvalUiState.v1";
const OLD_COLLAPSED_FOLDERS_KEY = "evalUiCollapsedFolders";
const URL_STATE_PREFIX = "#state=";
const HANDLE_HALF_SIZE = 5;
const HANDLE_HIT_RADIUS = 9;
const REVEAL_PAN_MS = 90;

function safeLocalStorage() {
  try {
    return window.localStorage || null;
  } catch {
    return null;
  }
}

function decodeUrlUiState() {
  if (!location.hash.startsWith(URL_STATE_PREFIX)) {
    return {};
  }
  try {
    return JSON.parse(decodeURIComponent(atob(location.hash.slice(URL_STATE_PREFIX.length))));
  } catch {
    return {};
  }
}

function encodeUrlUiState(payload) {
  return `${URL_STATE_PREFIX}${btoa(encodeURIComponent(JSON.stringify(payload)))}`;
}

function loadUiState() {
  const storage = safeLocalStorage();
  const urlState = decodeUrlUiState();
  try {
    const stored = storage ? JSON.parse(storage.getItem(UI_STATE_KEY) || "{}") : {};
    const oldCollapsed = storage ? JSON.parse(storage.getItem(OLD_COLLAPSED_FOLDERS_KEY) || "[]") : [];
    const collapsed = Array.isArray(urlState.collapsedFolders)
      ? urlState.collapsedFolders
      : Array.isArray(stored.collapsedFolders)
        ? stored.collapsedFolders
      : Array.isArray(oldCollapsed)
        ? oldCollapsed
        : [];
    const bundleState = {
      ...(stored.bundleState && typeof stored.bundleState === "object" ? stored.bundleState : {}),
      ...(urlState.bundleState && typeof urlState.bundleState === "object" ? urlState.bundleState : {}),
    };
    return {
      selectedBundlePath:
        typeof urlState.selectedBundlePath === "string"
          ? urlState.selectedBundlePath
          : typeof stored.selectedBundlePath === "string"
            ? stored.selectedBundlePath
            : null,
      search: typeof urlState.search === "string" ? urlState.search : typeof stored.search === "string" ? stored.search : "",
      mode: ["select", "draw-label", "draw-ignore"].includes(urlState.mode)
        ? urlState.mode
        : ["select", "draw-label", "draw-ignore"].includes(stored.mode)
          ? stored.mode
          : "select",
      tab: ["labels", "raw", "diff", "json"].includes(urlState.tab)
        ? urlState.tab
        : ["labels", "raw", "diff", "json"].includes(stored.tab)
          ? stored.tab
          : "labels",
      toggles:
        urlState.toggles && typeof urlState.toggles === "object"
          ? urlState.toggles
          : stored.toggles && typeof stored.toggles === "object"
            ? stored.toggles
            : {},
      collapsedFolders: new Set(collapsed.filter((value) => typeof value === "string")),
      bundleState,
    };
  } catch {
    storage?.removeItem(UI_STATE_KEY);
    return {
      selectedBundlePath: null,
      collapsedFolders: new Set(),
      bundleState: {},
    };
  }
}

const persistedUi = loadUiState();

const state = {
  index: null,
  bundles: [],
  selectedBundle: null,
  spec: null,
  labelExists: false,
  validationError: null,
  raw: null,
  report: null,
  mode: persistedUi.mode || "select",
  tab: persistedUi.tab || "labels",
  selected: null,
  dirty: false,
  formDirty: false,
  pendingApply: null,
  savedSpecJson: null,
  undoStack: [],
  redoStack: [],
  view: { scale: 1, x: 0, y: 0 },
  drag: null,
  panAnimation: null,
  collapsedFolders: persistedUi.collapsedFolders || new Set(),
  bundleState: persistedUi.bundleState || {},
  pendingSelectedBundlePath: persistedUi.selectedBundlePath || null,
  restoringUiState: false,
  drawStats: { labels: 0, characters: 0, ignored: 0, raw: 0, diffExpected: 0, diffActual: 0 },
};

function validView(view) {
  if (!view || typeof view !== "object") {
    return null;
  }
  const scale = Number(view.scale);
  const x = Number(view.x);
  const y = Number(view.y);
  if (!Number.isFinite(scale) || !Number.isFinite(x) || !Number.isFinite(y) || scale <= 0) {
    return null;
  }
  return {
    scale: Math.min(8, Math.max(0.02, scale)),
    x,
    y,
  };
}

function bundleUiState(bundlePath = state.selectedBundle?.bundle_path) {
  if (!bundlePath) {
    return {};
  }
  const stored = state.bundleState[bundlePath];
  return stored && typeof stored === "object" ? stored : {};
}

function rememberCurrentBundleState() {
  const bundlePath = state.selectedBundle?.bundle_path;
  if (!bundlePath) {
    return;
  }
  state.bundleState[bundlePath] = {
    view: { ...state.view },
    selected: cloneSelection(state.selected),
  };
}

function persistUiState() {
  const storage = safeLocalStorage();
  if (state.restoringUiState) {
    return;
  }
  rememberCurrentBundleState();
  const toggles = {
    labels: els.showLabels.checked,
    raw: els.showRaw.checked,
    diff: els.showDiff.checked,
    chars: els.showChars.checked,
    ignored: els.showIgnored.checked,
  };
  const payload = {
    selectedBundlePath: state.selectedBundle?.bundle_path || state.pendingSelectedBundlePath || null,
    search: els.searchBox.value,
    mode: state.mode,
    tab: state.tab,
    toggles,
    collapsedFolders: [...state.collapsedFolders].sort(),
    bundleState: state.bundleState,
  };
  try {
    storage?.setItem(UI_STATE_KEY, JSON.stringify(payload));
    storage?.removeItem(OLD_COLLAPSED_FOLDERS_KEY);
  } catch {
    // URL state is enough for restore; keep going if storage is unavailable or full.
  }
  const selectedPath = payload.selectedBundlePath;
  const hashPayload = {
    ...payload,
    bundleState:
      selectedPath && payload.bundleState[selectedPath]
        ? { [selectedPath]: payload.bundleState[selectedPath] }
        : {},
  };
  history.replaceState(null, "", `${location.pathname}${location.search}${encodeUrlUiState(hashPayload)}`);
}

function applyInitialUiState() {
  state.restoringUiState = true;
  els.searchBox.value = persistedUi.search || "";
  if (typeof persistedUi.toggles?.labels === "boolean") els.showLabels.checked = persistedUi.toggles.labels;
  if (typeof persistedUi.toggles?.raw === "boolean") els.showRaw.checked = persistedUi.toggles.raw;
  if (typeof persistedUi.toggles?.diff === "boolean") els.showDiff.checked = persistedUi.toggles.diff;
  if (typeof persistedUi.toggles?.chars === "boolean") els.showChars.checked = persistedUi.toggles.chars;
  if (typeof persistedUi.toggles?.ignored === "boolean") els.showIgnored.checked = persistedUi.toggles.ignored;
  setMode(state.mode);
  setTab(state.tab);
  state.restoringUiState = false;
}

function loadImageSource(src) {
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      cleanup();
      reject(new Error("Timed out loading image"));
    }, 15000);
    const cleanup = () => {
      window.clearTimeout(timeout);
      image.onload = null;
      image.onerror = null;
    };
    image.onload = () => {
      cleanup();
      resolve();
    };
    image.onerror = () => {
      cleanup();
      reject(new Error("Failed to load image"));
    };
    image.src = src;
    if (image.complete && image.naturalWidth) {
      cleanup();
      resolve();
    }
  });
}

function query(params) {
  return Object.entries(params)
    .map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`)
    .join("&");
}

async function apiJson(url, options = {}) {
  const response = await fetch(url, {
    ...options,
    headers: {
      ...(options.body && !(options.body instanceof Blob) ? { "Content-Type": "application/json" } : {}),
      ...(options.headers || {}),
    },
  });
  const text = await response.text();
  let data = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch (error) {
      throw new Error(text);
    }
  }
  if (!response.ok) {
    throw new Error(data?.error || response.statusText);
  }
  return data;
}

function setStatus(text, isError = false) {
  els.statusLine.textContent = text;
  els.statusLine.style.color = isError ? "var(--danger)" : "";
}

function cloneJson(value) {
  return value == null ? value : JSON.parse(JSON.stringify(value));
}

function serializeSpec(spec) {
  return JSON.stringify(spec || null);
}

function cloneSelection(selection) {
  return selection ? { kind: selection.kind, index: selection.index } : null;
}

function captureHistorySnapshot() {
  return {
    spec: cloneJson(state.spec),
    selected: cloneSelection(state.selected),
  };
}

function isSelectionValid(selection, spec = state.spec) {
  if (!selection || !spec) {
    return false;
  }
  if (selection.kind === "detection") {
    return selection.index >= 0 && selection.index < spec.detections.length;
  }
  if (selection.kind === "ignored") {
    return selection.index >= 0 && selection.index < spec.ignored.length;
  }
  return false;
}

function updateUndoRedoButtons() {
  els.undoBtn.disabled = !state.undoStack.length || Boolean(state.pendingApply);
  els.redoBtn.disabled = !state.redoStack.length || Boolean(state.pendingApply);
}

function resetHistory() {
  state.undoStack = [];
  state.redoStack = [];
  updateUndoRedoButtons();
}

function updateDirtyFromSpec() {
  state.dirty = Boolean(state.spec) && serializeSpec(state.spec) !== state.savedSpecJson;
  updateUndoRedoButtons();
  updateStatus();
}

function recordHistory(before = captureHistorySnapshot()) {
  if (!before.spec || !state.spec) {
    updateUndoRedoButtons();
    return;
  }
  if (serializeSpec(before.spec) === serializeSpec(state.spec)) {
    updateDirtyFromSpec();
    return;
  }
  state.undoStack.push(before);
  state.redoStack = [];
  updateDirtyFromSpec();
}

function restoreHistorySnapshot(snapshot) {
  state.spec = cloneJson(snapshot.spec);
  state.selected = isSelectionValid(snapshot.selected, state.spec) ? cloneSelection(snapshot.selected) : null;
  state.formDirty = false;
  syncJsonFromSpec();
  renderAll();
  updateDirtyFromSpec();
}

function undo() {
  if (!state.undoStack.length || state.pendingApply) {
    return;
  }
  const current = captureHistorySnapshot();
  const previous = state.undoStack.pop();
  state.redoStack.push(current);
  restoreHistorySnapshot(previous);
  updateStatus("undo");
}

function redo() {
  if (!state.redoStack.length || state.pendingApply) {
    return;
  }
  const current = captureHistorySnapshot();
  const next = state.redoStack.pop();
  state.undoStack.push(current);
  restoreHistorySnapshot(next);
  updateStatus("redo");
}

function markDirty() {
  updateDirtyFromSpec();
}

function clearDirty() {
  state.savedSpecJson = state.spec ? serializeSpec(state.spec) : null;
  state.dirty = false;
  updateUndoRedoButtons();
  updateStatus();
}

function updateStatus(extra = "") {
  if (!state.selectedBundle) {
    setStatus("No eval bundle loaded");
    updateDiagnostics();
    return;
  }
  const pieces = [];
  if (state.spec) {
    pieces.push(`${state.spec.detections.length} labels`);
    pieces.push(`${state.spec.ignored.length} ignored`);
  }
  if (state.raw) {
    pieces.push(`${state.raw.lines.length} raw`);
  }
  if (state.report) {
    pieces.push(`score ${(state.report.aggregate_score * 100).toFixed(1)}%`);
  }
  if (state.dirty) {
    pieces.push("unsaved");
  }
  if (state.formDirty) {
    pieces.push("form edits");
  }
  if (state.validationError) {
    pieces.push(`invalid: ${state.validationError}`);
  }
  if (extra) {
    pieces.push(extra);
  }
  setStatus(pieces.join(" | "));
  updateDiagnostics();
}

async function loadIndex() {
  els.imageList.textContent = "";
  setStatus("Loading eval index...");
  state.index = await apiJson("/api/bundles");
  state.bundles = state.index.bundles;
  els.rootLabel.textContent = state.index.root;
  els.imageStats.textContent = `${state.index.bundle_count} bundles, ${state.index.labeled_count} labeled`;
  renderBundleList();
  if (!state.selectedBundle && state.pendingSelectedBundlePath) {
    const restored = state.bundles.find((entry) => entry.bundle_path === state.pendingSelectedBundlePath);
    if (restored) {
      await selectBundle(restored, { revealFolder: false, restoring: true });
      return;
    }
    state.pendingSelectedBundlePath = null;
    persistUiState();
  }
  updateStatus();
}

function renderBundleList() {
  const filter = els.searchBox.value.trim().toLowerCase();
  const selectedPath = state.selectedBundle?.bundle_path;
  const hasFilter = Boolean(filter);
  const grouped = new Map();
  for (const entry of state.bundles) {
    const searchable = `${entry.bundle_path} ${entry.label_path || ""} ${entry.capture_path || ""}`.toLowerCase();
    if (filter && !searchable.includes(filter)) {
      continue;
    }
    const group = entry.collection || ".";
    if (!grouped.has(group)) {
      grouped.set(group, []);
    }
    grouped.get(group).push(entry);
  }
  els.imageList.textContent = "";
  for (const [directory, entries] of grouped) {
    const collapsed = !hasFilter && state.collapsedFolders.has(directory);
    const labeled = entries.filter((entry) => entry.labeled).length;
    const errors = entries.filter((entry) => entry.label_error).length;
    const folder = document.createElement("button");
    folder.type = "button";
    folder.className = "folder-row";
    folder.setAttribute("aria-expanded", String(!collapsed));
    folder.title = collapsed ? `Expand ${directory || "."}` : `Collapse ${directory || "."}`;

    const twisty = document.createElement("span");
    twisty.className = "folder-twisty";
    twisty.textContent = collapsed ? ">" : "v";

    const folderName = document.createElement("span");
    folderName.className = "folder-name";
    folderName.textContent = directory || ".";

    const folderMeta = document.createElement("span");
    folderMeta.className = `folder-meta${errors ? " error" : ""}`;
    folderMeta.textContent = `${entries.length} bundles | ${labeled} labeled${errors ? ` | ${errors} bad` : ""}`;

    folder.append(twisty, folderName, folderMeta);
    folder.addEventListener("click", () => toggleFolder(directory));
    els.imageList.append(folder);
    if (collapsed) {
      continue;
    }
    for (const entry of entries) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = `image-row${entry.bundle_path === selectedPath ? " selected" : ""}`;
      const main = document.createElement("div");
      const name = document.createElement("div");
      name.className = "image-name";
      name.textContent = entry.name;
      const sub = document.createElement("div");
      sub.className = "tagline";
      const dims = entry.width && entry.height ? `${entry.width}x${entry.height}` : "unknown size";
      sub.textContent = `${dims} | ${entry.detection_count} labels | ${entry.ignored_count} ignored`;
      main.append(name, sub);
      const badge = document.createElement("div");
      badge.className = `badge ${entry.label_error ? "error" : entry.labeled ? "good" : "warn"}`;
      badge.textContent = entry.label_error ? "bad" : entry.labeled ? "eval" : "new";
      row.append(main, badge);
      row.addEventListener("click", () => selectBundle(entry));
      els.imageList.append(row);
    }
  }
}

function saveCollapsedFolders() {
  persistUiState();
}

function toggleFolder(directory) {
  if (state.collapsedFolders.has(directory)) {
    state.collapsedFolders.delete(directory);
  } else {
    state.collapsedFolders.add(directory);
  }
  saveCollapsedFolders();
  renderBundleList();
  updateDiagnostics();
}

function scrollSelectedBundleIntoView() {
  requestAnimationFrame(() => {
    document.querySelector(".image-row.selected")?.scrollIntoView({ block: "nearest" });
  });
}

async function selectBundle(entry, options = {}) {
  if (state.dirty && !confirm("Discard unsaved label edits?")) {
    return;
  }
  rememberCurrentBundleState();
  if (options.revealFolder !== false && state.collapsedFolders.delete(entry.collection || ".")) {
    saveCollapsedFolders();
  }
  state.selectedBundle = entry;
  state.pendingSelectedBundlePath = entry.bundle_path;
  state.spec = null;
  state.raw = null;
  state.report = null;
  state.selected = null;
  state.savedSpecJson = null;
  resetHistory();
  state.dirty = false;
  state.validationError = null;
  els.selectedPath.textContent = entry.bundle_path;
  els.emptyState.style.display = "none";
  setStatus("Loading image...");
  renderBundleList();
  scrollSelectedBundleIntoView();
  if (!options.restoring) {
    persistUiState();
  }

  await loadImageSource(`/api/bundle-image?${query({ bundle: entry.bundle_path })}&v=${Date.now()}`);

  const label = await apiJson(`/api/label?${query({ bundle: entry.bundle_path })}`);
  state.spec = label.spec;
  state.savedSpecJson = serializeSpec(state.spec);
  state.labelExists = label.exists;
  state.validationError = label.validation_error;
  els.jsonEditor.value = label.raw;
  const stored = bundleUiState(entry.bundle_path);
  const restoredView = validView(stored.view);
  if (isSelectionValid(stored.selected, state.spec)) {
    state.selected = cloneSelection(stored.selected);
  }
  if (restoredView) {
    state.view = restoredView;
  } else {
    fitImage();
  }
  renderAll();
  clearDirty();
  scrollSelectedBundleIntoView();
  persistUiState();
}

async function navigateBundle(delta) {
  if (!state.selectedBundle) {
    return;
  }
  const collection = state.selectedBundle.collection || ".";
  const siblings = state.bundles.filter((entry) => (entry.collection || ".") === collection);
  const index = siblings.findIndex((entry) => entry.bundle_path === state.selectedBundle.bundle_path);
  if (index < 0) {
    return;
  }
  const next = siblings[index + delta];
  if (!next) {
    return;
  }
  await selectBundle(next);
}

function isTextEditingTarget(target) {
  if (!target) {
    return false;
  }
  const tag = target.tagName?.toLowerCase();
  return target.isContentEditable || tag === "input" || tag === "textarea" || tag === "select";
}

function fitImage() {
  if (!image.naturalWidth || !image.naturalHeight) {
    return;
  }
  cancelPanAnimation();
  const rect = els.canvasWrap.getBoundingClientRect();
  const scale = Math.min(rect.width / image.naturalWidth, rect.height / image.naturalHeight) * 0.96;
  state.view.scale = Math.max(scale, 0.01);
  state.view.x = (rect.width - image.naturalWidth * state.view.scale) / 2;
  state.view.y = (rect.height - image.naturalHeight * state.view.scale) / 2;
  draw();
  persistUiState();
}

function resizeCanvas() {
  const rect = els.canvasWrap.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.floor(rect.width * dpr));
  const height = Math.max(1, Math.floor(rect.height * dpr));
  if (els.canvas.width !== width || els.canvas.height !== height) {
    els.canvas.width = width;
    els.canvas.height = height;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

function imageToScreen(rect) {
  return {
    x: state.view.x + rect.x * state.view.scale,
    y: state.view.y + rect.y * state.view.scale,
    width: rect.width * state.view.scale,
    height: rect.height * state.view.scale,
  };
}

function pointToImage(x, y) {
  return {
    x: (x - state.view.x) / state.view.scale,
    y: (y - state.view.y) / state.view.scale,
  };
}

function easeOutCubic(t) {
  return 1 - (1 - t) ** 3;
}

function cancelPanAnimation() {
  if (!state.panAnimation) {
    return;
  }
  cancelAnimationFrame(state.panAnimation.frame);
  state.panAnimation = null;
}

function revealPaddingForViewport(width, height) {
  return Math.round(Math.min(72, Math.max(28, Math.min(width, height) * 0.08)));
}

function panDeltaToReveal(screenRect, viewportWidth, viewportHeight) {
  const padding = revealPaddingForViewport(viewportWidth, viewportHeight);
  const minX = padding;
  const minY = padding;
  const maxX = viewportWidth - padding;
  const maxY = viewportHeight - padding;
  const visibleWidth = Math.max(1, maxX - minX);
  const visibleHeight = Math.max(1, maxY - minY);
  const right = screenRect.x + screenRect.width;
  const bottom = screenRect.y + screenRect.height;
  let dx = 0;
  let dy = 0;

  if (screenRect.width > visibleWidth) {
    dx = viewportWidth / 2 - (screenRect.x + screenRect.width / 2);
  } else if (screenRect.x < minX) {
    dx = minX - screenRect.x;
  } else if (right > maxX) {
    dx = maxX - right;
  }

  if (screenRect.height > visibleHeight) {
    dy = viewportHeight / 2 - (screenRect.y + screenRect.height / 2);
  } else if (screenRect.y < minY) {
    dy = minY - screenRect.y;
  } else if (bottom > maxY) {
    dy = maxY - bottom;
  }

  return { dx, dy };
}

function animatePanBy(dx, dy) {
  if (Math.abs(dx) < 0.5 && Math.abs(dy) < 0.5) {
    return false;
  }
  cancelPanAnimation();
  const startX = state.view.x;
  const startY = state.view.y;
  const targetX = startX + dx;
  const targetY = startY + dy;
  const startedAt = performance.now();
  state.panAnimation = { frame: null };
  const step = (now) => {
    const t = Math.min(1, (now - startedAt) / REVEAL_PAN_MS);
    const eased = easeOutCubic(t);
    state.view.x = startX + (targetX - startX) * eased;
    state.view.y = startY + (targetY - startY) * eased;
    draw();
    if (t < 1 && state.panAnimation) {
      state.panAnimation.frame = requestAnimationFrame(step);
      return;
    }
    state.view.x = targetX;
    state.view.y = targetY;
    state.panAnimation = null;
    draw();
    persistUiState();
  };
  state.panAnimation.frame = requestAnimationFrame(step);
  return true;
}

function revealRectInViewport(imageRect) {
  if (!imageRect || !image.naturalWidth) {
    return;
  }
  const viewport = els.canvasWrap.getBoundingClientRect();
  const delta = panDeltaToReveal(imageToScreen(imageRect), viewport.width, viewport.height);
  animatePanBy(delta.dx, delta.dy);
}

function revealSelectedInViewport() {
  revealRectInViewport(rectForSelection(state.selected));
}

function draw() {
  resizeCanvas();
  const drawStats = { labels: 0, characters: 0, ignored: 0, raw: 0, diffExpected: 0, diffActual: 0 };
  const rect = els.canvasWrap.getBoundingClientRect();
  ctx.clearRect(0, 0, rect.width, rect.height);
  if (!image.naturalWidth) {
    state.drawStats = drawStats;
    updateDiagnostics();
    return;
  }

  ctx.imageSmoothingEnabled = true;
  ctx.drawImage(
    image,
    state.view.x,
    state.view.y,
    image.naturalWidth * state.view.scale,
    image.naturalHeight * state.view.scale,
  );

  if (els.showLabels.checked && state.spec) {
    drawLabels(drawStats);
  }
  if (els.showChars.checked && state.spec) {
    drawCharacters(drawStats);
  }
  if (els.showRaw.checked && state.raw) {
    drawRaw(drawStats);
  }
  if (els.showDiff.checked && state.report) {
    drawDiff(drawStats);
  }
  if (state.drag?.preview) {
    drawRect(state.drag.preview, "rgba(240,180,76,0.18)", "rgba(240,180,76,0.95)", "new", false);
  }
  state.drawStats = drawStats;
  updateDiagnostics();
}

function drawLabels(drawStats) {
  state.spec.detections.forEach((det, index) => {
    drawStats.labels += 1;
    const selected = state.selected?.kind === "detection" && state.selected.index === index;
    drawRect(
      det.bounds,
      selected ? "rgba(240,180,76,0.22)" : "rgba(88,196,143,0.15)",
      selected ? "rgba(240,180,76,1)" : "rgba(88,196,143,0.95)",
      det.id || det.text,
      false,
    );
    if (selected) {
      drawHandles(det.bounds);
    }
  });
  if (els.showIgnored.checked) {
    state.spec.ignored.forEach((ignored, index) => {
      if (!ignored.bounds) {
        return;
      }
      drawStats.ignored += 1;
      const selected = state.selected?.kind === "ignored" && state.selected.index === index;
      drawRect(
        ignored.bounds,
        selected ? "rgba(240,180,76,0.18)" : "rgba(170,162,154,0.13)",
        selected ? "rgba(240,180,76,1)" : "rgba(170,162,154,0.85)",
        ignored.text || "ignored",
        true,
      );
      if (selected) {
        drawHandles(ignored.bounds);
      }
    });
  }
}

function drawCharacters(drawStats) {
  state.spec.detections.forEach((det) => {
    for (const ch of det.characters || []) {
      drawStats.characters += 1;
      drawRect(ch.bounds, "rgba(255,255,255,0.04)", "rgba(255,255,255,0.45)", ch.text, true);
    }
  });
}

function drawRaw(drawStats) {
  state.raw.lines.forEach((line, index) => {
    drawStats.raw += 1;
    drawRect(
      line.text_box.rect,
      "rgba(71,184,214,0.14)",
      "rgba(71,184,214,0.95)",
      `${index + 1}: ${line.text}`,
      false,
    );
  });
}

function drawDiff(drawStats) {
  for (const score of state.report.detections) {
    drawStats.diffExpected += 1;
    const ok = score.score >= 0.999;
    const color = ok ? "rgba(88,196,143,0.95)" : score.actual ? "rgba(240,180,76,1)" : "rgba(239,106,106,1)";
    const fill = ok ? "rgba(88,196,143,0.10)" : "rgba(239,106,106,0.12)";
    drawRect(score.expected_bounds, fill, color, `${score.id} ${(score.score * 100).toFixed(0)}%`, !ok);
    if (score.actual) {
      drawStats.diffActual += 1;
      drawRect(score.actual.text_box.rect, "rgba(240,180,76,0.08)", "rgba(240,180,76,0.9)", score.actual.text, true);
    }
  }
  for (const actual of state.report.unexpected_actual || []) {
    drawStats.diffActual += 1;
    drawRect(actual.text_box.rect, "rgba(219,120,207,0.16)", "rgba(219,120,207,0.95)", actual.text, false);
  }
  for (const actual of state.report.ignored_actual || []) {
    drawStats.diffActual += 1;
    drawRect(actual.text_box.rect, "rgba(170,162,154,0.12)", "rgba(170,162,154,0.8)", actual.text, true);
  }
}

function drawRect(imageRect, fill, stroke, label, dashed) {
  const r = imageToScreen(imageRect);
  if (r.width < 1 || r.height < 1) {
    return;
  }
  ctx.save();
  ctx.fillStyle = fill;
  ctx.strokeStyle = stroke;
  ctx.lineWidth = 2;
  ctx.setLineDash(dashed ? [6, 4] : []);
  ctx.fillRect(r.x, r.y, r.width, r.height);
  ctx.strokeRect(r.x, r.y, r.width, r.height);
  if (label) {
    const text = String(label).slice(0, 48);
    ctx.font = "12px Segoe UI, sans-serif";
    const metrics = ctx.measureText(text);
    const labelWidth = Math.min(metrics.width + 10, Math.max(36, r.width));
    const labelY = Math.max(0, r.y - 18);
    ctx.fillStyle = "rgba(17,17,17,0.82)";
    ctx.fillRect(r.x, labelY, labelWidth, 18);
    ctx.fillStyle = "#f3efe8";
    ctx.fillText(text, r.x + 5, labelY + 13);
  }
  ctx.restore();
}

function drawHandles(imageRect) {
  const r = imageToScreen(imageRect);
  ctx.save();
  ctx.fillStyle = "#f0b44c";
  ctx.strokeStyle = "#111";
  for (const { x, y } of handlePoints(r)) {
    ctx.beginPath();
    ctx.rect(x - HANDLE_HALF_SIZE, y - HANDLE_HALF_SIZE, HANDLE_HALF_SIZE * 2, HANDLE_HALF_SIZE * 2);
    ctx.fill();
    ctx.stroke();
  }
  ctx.restore();
}

function renderAll() {
  renderLabels();
  renderRaw();
  renderDiff();
  updateForm();
  draw();
  updateStatus();
  persistUiState();
}

function renderLabels() {
  els.labelList.textContent = "";
  if (!state.spec) {
    els.labelSummary.textContent = "No label file loaded.";
    return;
  }
  els.labelSummary.textContent = `${state.spec.detections.length} detections, ${state.spec.ignored.length} ignored`;
  state.spec.detections.forEach((det, index) => {
    const row = itemButton(det.id || `label ${index + 1}`, det.text, state.selected?.kind === "detection" && state.selected.index === index);
    row.addEventListener("click", () => selectItem("detection", index));
    els.labelList.append(row);
  });
  state.spec.ignored.forEach((ignored, index) => {
    const row = itemButton(`ignored ${index + 1}`, ignored.text || ignored.reason, state.selected?.kind === "ignored" && state.selected.index === index);
    row.addEventListener("click", () => selectItem("ignored", index));
    els.labelList.append(row);
  });
}

function itemButton(title, subtitle, selected) {
  const row = document.createElement("button");
  row.type = "button";
  fillItemRow(row, title, subtitle, selected);
  return row;
}

function itemDiv(title, subtitle, selected) {
  const row = document.createElement("div");
  fillItemRow(row, title, subtitle, selected);
  return row;
}

function fillItemRow(row, title, subtitle, selected) {
  row.className = `item-row${selected ? " selected" : ""}`;
  const titleEl = document.createElement("div");
  titleEl.className = "item-title";
  titleEl.textContent = title || "(untitled)";
  const subEl = document.createElement("div");
  subEl.className = "item-subtitle";
  subEl.textContent = subtitle || "";
  row.append(titleEl, subEl);
}

function selectItem(kind, index) {
  state.selected = { kind, index };
  state.formDirty = false;
  setTab("labels");
  renderAll();
  revealSelectedInViewport();
}

function selectedObject() {
  if (!state.spec || !state.selected) {
    return null;
  }
  if (state.selected.kind === "detection") {
    return state.spec.detections[state.selected.index] || null;
  }
  if (state.selected.kind === "ignored") {
    return state.spec.ignored[state.selected.index] || null;
  }
  return null;
}

function updateForm() {
  const obj = selectedObject();
  const hasSelection = Boolean(obj);
  const applying = Boolean(state.pendingApply);
  for (const input of els.labelForm.querySelectorAll("input, textarea, button")) {
    input.disabled = !hasSelection || applying;
  }
  els.applyFormBtn.textContent = applying ? "Applying..." : "Apply";
  if (!obj) {
    els.selKind.value = "";
    els.detId.value = "";
    els.detText.value = "";
    els.ignoreReason.value = "";
    els.rectX.value = "";
    els.rectY.value = "";
    els.rectW.value = "";
    els.rectH.value = "";
    els.tolX.value = "";
    els.tolY.value = "";
    els.tolW.value = "";
    els.tolH.value = "";
    els.detNotes.value = "";
    return;
  }
  const isDetection = state.selected.kind === "detection";
  els.selKind.value = state.selected.kind;
  els.detId.disabled = !isDetection || applying;
  els.detText.disabled = applying;
  els.ignoreReason.disabled = isDetection || applying;
  els.rebuildBtn.disabled = !isDetection || applying;
  els.detNotes.disabled = !isDetection || applying;
  els.detId.value = isDetection ? obj.id || "" : "";
  els.detText.value = obj.text || "";
  els.ignoreReason.value = isDetection ? "" : obj.reason || "";
  const r = isDetection ? obj.bounds : obj.bounds || { x: 0, y: 0, width: 1, height: 1 };
  els.rectX.value = fmtNum(r.x);
  els.rectY.value = fmtNum(r.y);
  els.rectW.value = fmtNum(r.width);
  els.rectH.value = fmtNum(r.height);
  const tol = isDetection ? obj.bounds_tolerance : null;
  els.tolX.value = tol ? fmtNum(tol.x) : "";
  els.tolY.value = tol ? fmtNum(tol.y) : "";
  els.tolW.value = tol ? fmtNum(tol.width) : "";
  els.tolH.value = tol ? fmtNum(tol.height) : "";
  els.detNotes.value = isDetection ? obj.notes || "" : "";
}

function fmtNum(value) {
  return Number.isFinite(value) ? Number(value.toFixed(2)).toString() : "";
}

function formRect() {
  return {
    x: Number(els.rectX.value || 0),
    y: Number(els.rectY.value || 0),
    width: Math.max(1, Number(els.rectW.value || 1)),
    height: Math.max(1, Number(els.rectH.value || 1)),
  };
}

function formTolerance() {
  const values = [els.tolX.value, els.tolY.value, els.tolW.value, els.tolH.value];
  if (values.every((value) => value.trim() === "")) {
    return null;
  }
  return {
    x: Math.max(0, Number(els.tolX.value || 0)),
    y: Math.max(0, Number(els.tolY.value || 0)),
    width: Math.max(0, Number(els.tolW.value || 0)),
    height: Math.max(0, Number(els.tolH.value || 0)),
  };
}

async function applyForm() {
  if (state.pendingApply) {
    return state.pendingApply;
  }
  if (!state.selected || !state.spec) {
    return;
  }
  const before = captureHistorySnapshot();
  const selection = { ...state.selected };
  const isDetection = selection.kind === "detection";
  const draft = isDetection
    ? {
        id: els.detId.value.trim() || nextLabelId(),
        text: els.detText.value || "TODO",
        bounds: formRect(),
        bounds_tolerance: formTolerance(),
        notes: els.detNotes.value.trim() || null,
      }
    : null;
  const ignoredDraft = isDetection
    ? null
    : {
        text: els.detText.value,
        reason: els.ignoreReason.value.trim() || "ignored region",
        bounds: formRect(),
      };
  const operation = Promise.resolve().then(async () => {
    els.saveBtn.disabled = true;
    setFormBusy(true);
    setStatus(isDetection ? "Rebuilding label metadata..." : "Applying ignored region...");
    if (isDetection) {
      const response = await apiJson("/api/synthesize", {
        method: "POST",
        body: JSON.stringify(draft),
      });
      state.spec.detections[selection.index] = response.detection;
    } else {
      state.spec.ignored[selection.index] = {
        ...state.spec.ignored[selection.index],
        ...ignoredDraft,
      };
    }
    state.formDirty = false;
    recordHistory(before);
    syncJsonFromSpec();
    renderAll();
  });
  state.pendingApply = operation;
  try {
    await operation;
  } catch (error) {
    setStatus(error.message, true);
    throw error;
  } finally {
    state.pendingApply = null;
    els.saveBtn.disabled = false;
    setFormBusy(false);
    updateForm();
    updateStatus();
  }
}

function setFormBusy(isBusy) {
  for (const input of els.labelForm.querySelectorAll("input, textarea, button")) {
    if (input.id !== "selKind") {
      input.disabled = isBusy || !state.selected;
    }
  }
  els.applyFormBtn.textContent = isBusy ? "Applying..." : "Apply";
  updateUndoRedoButtons();
}

function syncJsonFromSpec() {
  if (state.spec) {
    els.jsonEditor.value = JSON.stringify(state.spec, null, 2);
  }
}

function syncSpecFromJson() {
  state.spec = JSON.parse(els.jsonEditor.value);
}

async function saveLabels() {
  if (!state.selectedBundle || !state.spec) {
    return;
  }
  if (state.formDirty || state.pendingApply) {
    await applyForm();
  }
  if (state.pendingApply) {
    setStatus("Waiting for label metadata...");
    await state.pendingApply;
  }
  els.saveBtn.disabled = true;
  try {
    const response = await apiJson(`/api/label?${query({ bundle: state.selectedBundle.bundle_path })}`, {
      method: "PUT",
      body: JSON.stringify(state.spec),
    });
    state.spec = response.spec;
    state.validationError = response.validation_error;
    state.labelExists = response.exists;
    els.jsonEditor.value = response.raw;
    state.report = null;
    await loadIndex();
    renderAll();
    clearDirty();
    setStatus("Saved labels");
  } catch (error) {
    setStatus(error.message, true);
    throw error;
  } finally {
    els.saveBtn.disabled = false;
  }
}

async function saveJson() {
  const before = captureHistorySnapshot();
  syncSpecFromJson();
  recordHistory(before);
  await saveLabels();
}

function nextLabelId() {
  const used = new Set((state.spec?.detections || []).map((det) => det.id));
  for (let i = 1; i < 10000; i += 1) {
    const id = `label_${String(i).padStart(2, "0")}`;
    if (!used.has(id)) {
      return id;
    }
  }
  return `label_${Date.now()}`;
}

function defaultRect() {
  if (!image.naturalWidth) {
    return { x: 0, y: 0, width: 120, height: 36 };
  }
  const wrap = els.canvasWrap.getBoundingClientRect();
  const center = pointToImage(wrap.width / 2, wrap.height / 2);
  return clampRect({
    x: center.x - 90,
    y: center.y - 20,
    width: 180,
    height: 40,
  });
}

async function addLabel(rect = defaultRect(), text = "TODO") {
  if (!state.spec) {
    return;
  }
  const before = captureHistorySnapshot();
  const draft = {
    id: nextLabelId(),
    text,
    bounds: clampRect(rect),
    bounds_tolerance: null,
    notes: null,
  };
  const response = await apiJson("/api/synthesize", {
    method: "POST",
    body: JSON.stringify(draft),
  });
  state.spec.detections.push(response.detection);
  state.selected = { kind: "detection", index: state.spec.detections.length - 1 };
  recordHistory(before);
  syncJsonFromSpec();
  renderAll();
}

function addIgnored(rect = defaultRect()) {
  if (!state.spec) {
    return;
  }
  const before = captureHistorySnapshot();
  state.spec.ignored.push({
    text: "",
    reason: "ignored region",
    bounds: clampRect(rect),
  });
  state.selected = { kind: "ignored", index: state.spec.ignored.length - 1 };
  recordHistory(before);
  syncJsonFromSpec();
  renderAll();
}

function deleteSelected() {
  if (!state.selected || !state.spec) {
    return;
  }
  const before = captureHistorySnapshot();
  if (state.selected.kind === "detection") {
    state.spec.detections.splice(state.selected.index, 1);
  } else {
    state.spec.ignored.splice(state.selected.index, 1);
  }
  state.selected = null;
  state.formDirty = false;
  recordHistory(before);
  syncJsonFromSpec();
  renderAll();
}

async function runOcr() {
  if (!state.selectedBundle) {
    return;
  }
  setStatus("Running OCR...");
  els.runOcrBtn.disabled = true;
  try {
    const response = await apiJson(`/api/detections?${query({ bundle: state.selectedBundle.bundle_path })}`, {
      method: "POST",
      body: "{}",
    });
    state.raw = response.result;
    state.report = response.report;
    els.showRaw.checked = true;
    if (state.report) {
      els.showDiff.checked = true;
    }
    renderAll();
    setStatus("OCR complete");
  } catch (error) {
    setStatus(error.message, true);
  } finally {
    els.runOcrBtn.disabled = false;
  }
}

function renderRaw() {
  els.rawList.textContent = "";
  if (!state.raw) {
    els.rawSummary.textContent = "Run OCR to see raw detections.";
    return;
  }
  els.rawSummary.textContent = `${state.raw.lines.length} raw detections`;
  state.raw.lines.forEach((line, index) => {
    const row = itemDiv(`${index + 1}. ${line.text}`, `conf ${(line.confidence * 100).toFixed(1)}% | x ${line.text_box.rect.x.toFixed(0)} y ${line.text_box.rect.y.toFixed(0)}`, false);
    const adopt = document.createElement("button");
    adopt.type = "button";
    adopt.textContent = "Adopt";
    adopt.addEventListener("click", (event) => {
      event.stopPropagation();
      addLabel(line.text_box.rect, line.text);
    });
    row.append(adopt);
    row.addEventListener("click", () => {
      state.selected = null;
      centerOnRect(line.text_box.rect);
      draw();
    });
    els.rawList.append(row);
  });
}

function renderDiff() {
  els.diffList.textContent = "";
  if (!state.report) {
    els.diffSummary.textContent = "Run OCR to score labels.";
    return;
  }
  const report = state.report;
  const cls = report.passed ? "score-pass" : "score-fail";
  els.diffSummary.innerHTML = `<span class="${cls}">${(report.aggregate_score * 100).toFixed(1)}%</span> | matched ${report.matched_detection_count}/${report.expected_detection_count} | unexpected ${report.unexpected_actual_count}`;
  report.detections.forEach((score, index) => {
    const subtitle = score.actual
      ? `actual: ${score.actual.text} | chars ${(score.character_score * 100).toFixed(0)}% | meta ${(score.metadata_score * 100).toFixed(0)}%`
      : "actual: none";
    const row = itemButton(`${score.id} ${(score.score * 100).toFixed(0)}%`, subtitle, false);
    row.addEventListener("click", () => selectItem("detection", index));
    els.diffList.append(row);
  });
  for (const actual of report.unexpected_actual || []) {
    const row = itemDiv(`unexpected: ${actual.text}`, `x ${actual.text_box.rect.x.toFixed(0)} y ${actual.text_box.rect.y.toFixed(0)}`, false);
    const adopt = document.createElement("button");
    adopt.type = "button";
    adopt.textContent = "Adopt";
    adopt.addEventListener("click", (event) => {
      event.stopPropagation();
      addLabel(actual.text_box.rect, actual.text);
    });
    row.append(adopt);
    els.diffList.append(row);
  }
}

function centerOnRect(rect) {
  cancelPanAnimation();
  const wrap = els.canvasWrap.getBoundingClientRect();
  const cx = rect.x + rect.width / 2;
  const cy = rect.y + rect.height / 2;
  state.view.x = wrap.width / 2 - cx * state.view.scale;
  state.view.y = wrap.height / 2 - cy * state.view.scale;
  persistUiState();
}

function setMode(mode) {
  state.mode = mode;
  els.selectModeBtn.classList.toggle("active", mode === "select");
  els.drawLabelBtn.classList.toggle("active", mode === "draw-label");
  els.drawIgnoreBtn.classList.toggle("active", mode === "draw-ignore");
  updateCanvasCursor();
  persistUiState();
}

function setTab(tab) {
  state.tab = tab;
  for (const [name, button, panel] of [
    ["labels", els.tabLabels, els.labelsPanel],
    ["raw", els.tabRaw, els.rawPanel],
    ["diff", els.tabDiff, els.diffPanel],
    ["json", els.tabJson, els.jsonPanel],
  ]) {
    button.classList.toggle("active", tab === name);
    panel.classList.toggle("active", tab === name);
  }
  persistUiState();
}

function hitTestSelection(point) {
  if (!state.spec) {
    return null;
  }
  if (els.showLabels.checked) {
    for (let index = state.spec.detections.length - 1; index >= 0; index -= 1) {
      if (contains(state.spec.detections[index].bounds, point)) {
        return { kind: "detection", index };
      }
    }
  }
  if (els.showIgnored.checked) {
    for (let index = state.spec.ignored.length - 1; index >= 0; index -= 1) {
      const bounds = state.spec.ignored[index].bounds;
      if (bounds && contains(bounds, point)) {
        return { kind: "ignored", index };
      }
    }
  }
  return null;
}

function contains(rect, point) {
  return point.x >= rect.x && point.x <= rect.x + rect.width && point.y >= rect.y && point.y <= rect.y + rect.height;
}

function rectForSelection(selection) {
  if (!selection || !state.spec) {
    return null;
  }
  if (selection.kind === "detection") {
    return state.spec.detections[selection.index]?.bounds || null;
  }
  if (selection.kind === "ignored") {
    return state.spec.ignored[selection.index]?.bounds || null;
  }
  return null;
}

function handlePoints(screenRect) {
  const midX = screenRect.x + screenRect.width / 2;
  const midY = screenRect.y + screenRect.height / 2;
  const right = screenRect.x + screenRect.width;
  const bottom = screenRect.y + screenRect.height;
  return [
    { name: "nw", x: screenRect.x, y: screenRect.y },
    { name: "n", x: midX, y: screenRect.y },
    { name: "ne", x: right, y: screenRect.y },
    { name: "e", x: right, y: midY },
    { name: "se", x: right, y: bottom },
    { name: "s", x: midX, y: bottom },
    { name: "sw", x: screenRect.x, y: bottom },
    { name: "w", x: screenRect.x, y: midY },
  ];
}

function handleForScreenRect(screenRect, screenPoint) {
  for (const { name, x, y } of handlePoints(screenRect)) {
    if (Math.abs(screenPoint.x - x) <= HANDLE_HIT_RADIUS && Math.abs(screenPoint.y - y) <= HANDLE_HIT_RADIUS) {
      return name;
    }
  }
  return null;
}

function cursorForHandle(handle) {
  if (handle === "n" || handle === "s") return "ns-resize";
  if (handle === "e" || handle === "w") return "ew-resize";
  if (handle === "nw" || handle === "se") return "nwse-resize";
  if (handle === "ne" || handle === "sw") return "nesw-resize";
  if (handle === "move") return "move";
  return null;
}

function visibleEditableSelections() {
  if (!state.spec) {
    return [];
  }
  const selections = [];
  if (els.showLabels.checked) {
    for (let index = state.spec.detections.length - 1; index >= 0; index -= 1) {
      selections.push({ kind: "detection", index });
    }
  }
  if (els.showIgnored.checked) {
    for (let index = state.spec.ignored.length - 1; index >= 0; index -= 1) {
      if (state.spec.ignored[index].bounds) {
        selections.push({ kind: "ignored", index });
      }
    }
  }
  return selections;
}

function sameSelection(left, right) {
  return Boolean(left && right && left.kind === right.kind && left.index === right.index);
}

function isSelectionVisible(selection) {
  if (!selection) {
    return false;
  }
  if (selection.kind === "detection") {
    return els.showLabels.checked;
  }
  if (selection.kind === "ignored") {
    return els.showIgnored.checked;
  }
  return false;
}

function hitTestResizeHandle(screenPoint) {
  const selections = [];
  if (isSelectionVisible(state.selected) && isSelectionValid(state.selected, state.spec) && rectForSelection(state.selected)) {
    selections.push(cloneSelection(state.selected));
  }
  for (const selection of visibleEditableSelections()) {
    if (!sameSelection(selection, state.selected)) {
      selections.push(selection);
    }
  }
  for (const selection of selections) {
    const rect = rectForSelection(selection);
    if (!rect) {
      continue;
    }
    const handle = handleForScreenRect(imageToScreen(rect), screenPoint);
    if (handle) {
      return { selection, handle };
    }
  }
  return null;
}

function updateCanvasCursor(screenPoint = null) {
  if (!image.naturalWidth) {
    els.canvas.style.cursor = "default";
    return;
  }
  if (state.drag?.type === "pan") {
    els.canvas.style.cursor = "grabbing";
    return;
  }
  if (state.drag?.type === "edit") {
    els.canvas.style.cursor = cursorForHandle(state.drag.handle) || "move";
    return;
  }
  if (!screenPoint) {
    els.canvas.style.cursor = state.mode === "select" ? "grab" : "crosshair";
    return;
  }
  const handleHit = hitTestResizeHandle(screenPoint);
  if (handleHit) {
    els.canvas.style.cursor = cursorForHandle(handleHit.handle) || "move";
    return;
  }
  const imagePoint = pointToImage(screenPoint.x, screenPoint.y);
  if (state.mode === "select" && hitTestSelection(imagePoint)) {
    els.canvas.style.cursor = "move";
    return;
  }
  els.canvas.style.cursor = state.mode === "select" ? "grab" : "crosshair";
}

function beginEditDrag(selection, handle, imagePoint) {
  state.selected = cloneSelection(selection);
  const rect = rectForSelection(state.selected);
  if (!rect) {
    return false;
  }
  state.drag = {
    type: "edit",
    handle,
    start: imagePoint,
    original: { ...rect },
    before: captureHistorySnapshot(),
  };
  renderAll();
  updateCanvasCursor();
  return true;
}

function canvasPoint(event) {
  const rect = els.canvas.getBoundingClientRect();
  return { x: event.clientX - rect.left, y: event.clientY - rect.top };
}

els.canvas.addEventListener("pointerdown", (event) => {
  if (!image.naturalWidth) {
    return;
  }
  cancelPanAnimation();
  els.canvas.setPointerCapture(event.pointerId);
  const screen = canvasPoint(event);
  const imagePoint = pointToImage(screen.x, screen.y);
  const handleHit = hitTestResizeHandle(screen);
  if (handleHit && beginEditDrag(handleHit.selection, handleHit.handle, imagePoint)) {
    return;
  }
  if (state.mode === "draw-label" || state.mode === "draw-ignore") {
    state.drag = {
      type: state.mode,
      start: imagePoint,
      preview: { x: imagePoint.x, y: imagePoint.y, width: 1, height: 1 },
    };
    draw();
    return;
  }

  const hit = hitTestSelection(imagePoint);
  if (hit) {
    beginEditDrag(hit, "move", imagePoint);
  } else {
    state.selected = null;
    state.drag = {
      type: "pan",
      start: screen,
      original: { x: state.view.x, y: state.view.y },
    };
    renderAll();
    updateCanvasCursor();
  }
});

els.canvas.addEventListener("pointermove", (event) => {
  if (!state.drag) {
    updateCanvasCursor(canvasPoint(event));
    return;
  }
  const screen = canvasPoint(event);
  const imagePoint = pointToImage(screen.x, screen.y);
  if (state.drag.type === "pan") {
    state.view.x = state.drag.original.x + screen.x - state.drag.start.x;
    state.view.y = state.drag.original.y + screen.y - state.drag.start.y;
    draw();
    return;
  }
  if (state.drag.type === "draw-label" || state.drag.type === "draw-ignore") {
    state.drag.preview = rectFromPoints(state.drag.start, imagePoint);
    draw();
    return;
  }
  if (state.drag.type === "edit") {
    const obj = selectedObject();
    if (!obj) {
      return;
    }
    const next = resizeRect(state.drag.original, state.drag.start, imagePoint, state.drag.handle);
    if (state.selected.kind === "detection") {
      obj.bounds = clampRect(next);
    } else {
      obj.bounds = clampRect(next);
    }
    updateForm();
    draw();
  }
});

els.canvas.addEventListener("pointerup", async (event) => {
  if (!state.drag) {
    return;
  }
  els.canvas.releasePointerCapture(event.pointerId);
  const drag = state.drag;
  state.drag = null;
  updateCanvasCursor(canvasPoint(event));
  if (drag.type === "draw-label" || drag.type === "draw-ignore") {
    const rect = clampRect(drag.preview);
    if (rect.width > 4 && rect.height > 4) {
      if (drag.type === "draw-label") {
        await addLabel(rect);
      } else {
        addIgnored(rect);
      }
    } else {
      draw();
    }
    return;
  }
  if (drag.type === "pan") {
    persistUiState();
    updateDiagnostics();
    return;
  }
  if (drag.type === "edit" && state.selected) {
    if (state.selected.kind === "detection") {
      const det = selectedObject();
      const response = await apiJson("/api/synthesize", {
        method: "POST",
        body: JSON.stringify({
          id: det.id,
          text: det.text,
          bounds: det.bounds,
          bounds_tolerance: det.bounds_tolerance || null,
          notes: det.notes || null,
        }),
      });
      state.spec.detections[state.selected.index] = response.detection;
    }
    recordHistory(drag.before);
    syncJsonFromSpec();
    renderAll();
  }
});

els.canvas.addEventListener("wheel", (event) => {
  if (!image.naturalWidth) {
    return;
  }
  cancelPanAnimation();
  event.preventDefault();
  const screen = canvasPoint(event);
  const before = pointToImage(screen.x, screen.y);
  const factor = event.deltaY < 0 ? 1.12 : 1 / 1.12;
  state.view.scale = Math.min(8, Math.max(0.02, state.view.scale * factor));
  state.view.x = screen.x - before.x * state.view.scale;
  state.view.y = screen.y - before.y * state.view.scale;
  draw();
  persistUiState();
}, { passive: false });

function rectFromPoints(a, b) {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    width: Math.abs(a.x - b.x),
    height: Math.abs(a.y - b.y),
  };
}

function resizeRect(original, start, point, handle) {
  const dx = point.x - start.x;
  const dy = point.y - start.y;
  let x = original.x;
  let y = original.y;
  let right = original.x + original.width;
  let bottom = original.y + original.height;
  if (handle === "move") {
    x += dx;
    y += dy;
    right += dx;
    bottom += dy;
  } else {
    if (handle.includes("w")) x += dx;
    if (handle.includes("e")) right += dx;
    if (handle.includes("n")) y += dy;
    if (handle.includes("s")) bottom += dy;
  }
  if (right < x) [x, right] = [right, x];
  if (bottom < y) [y, bottom] = [bottom, y];
  return { x, y, width: right - x, height: bottom - y };
}

function clampRect(rect) {
  const maxW = image.naturalWidth || 100000;
  const maxH = image.naturalHeight || 100000;
  const x = Math.max(0, Math.min(maxW - 1, rect.x));
  const y = Math.max(0, Math.min(maxH - 1, rect.y));
  const right = Math.max(x + 1, Math.min(maxW, rect.x + rect.width));
  const bottom = Math.max(y + 1, Math.min(maxH, rect.y + rect.height));
  return { x, y, width: right - x, height: bottom - y };
}

async function uploadSelectedFile() {
  const file = els.uploadInput.files?.[0];
  if (!file) {
    return;
  }
  const dir = state.selectedBundle?.collection || "";
  const targetDir = dir === "." ? "" : dir;
  setStatus("Uploading image...");
  try {
    const entry = await apiJson(`/api/upload?${query({ dir: targetDir, name: file.name })}`, {
      method: "POST",
      body: file,
      headers: { "Content-Type": file.type || "application/octet-stream" },
    });
    await loadIndex();
    const fresh = state.bundles.find((bundleEntry) => bundleEntry.bundle_path === entry.bundle_path) || entry;
    await selectBundle(fresh);
  } catch (error) {
    setStatus(error.message, true);
  } finally {
    els.uploadInput.value = "";
  }
}

els.reloadBtn.addEventListener("click", loadIndex);
els.searchBox.addEventListener("input", () => {
  renderBundleList();
  persistUiState();
});
els.uploadInput.addEventListener("change", uploadSelectedFile);
els.runOcrBtn.addEventListener("click", runOcr);
els.fitBtn.addEventListener("click", fitImage);
els.selectModeBtn.addEventListener("click", () => setMode("select"));
els.drawLabelBtn.addEventListener("click", () => setMode("draw-label"));
els.drawIgnoreBtn.addEventListener("click", () => setMode("draw-ignore"));
els.tabLabels.addEventListener("click", () => setTab("labels"));
els.tabRaw.addEventListener("click", () => setTab("raw"));
els.tabDiff.addEventListener("click", () => setTab("diff"));
els.tabJson.addEventListener("click", () => setTab("json"));
els.addLabelBtn.addEventListener("click", () => addLabel());
els.addIgnoreBtn.addEventListener("click", () => addIgnored());
els.undoBtn.addEventListener("click", undo);
els.redoBtn.addEventListener("click", redo);
els.saveBtn.addEventListener("click", saveLabels);
els.applyFormBtn.addEventListener("click", applyForm);
els.rebuildBtn.addEventListener("click", applyForm);
els.deleteBtn.addEventListener("click", deleteSelected);
for (const input of els.labelForm.querySelectorAll("input, textarea")) {
  if (input.id === "selKind") {
    continue;
  }
  input.addEventListener("input", () => {
    if (state.selected && !state.pendingApply) {
      state.formDirty = true;
      updateStatus();
    }
  });
}
els.formatJsonBtn.addEventListener("click", () => {
  const before = captureHistorySnapshot();
  syncSpecFromJson();
  recordHistory(before);
  state.formDirty = false;
  syncJsonFromSpec();
  renderAll();
});
els.saveJsonBtn.addEventListener("click", saveJson);

for (const checkbox of [els.showLabels, els.showRaw, els.showDiff, els.showChars, els.showIgnored]) {
  checkbox.addEventListener("change", () => {
    draw();
    persistUiState();
  });
}

window.addEventListener("resize", draw);
window.addEventListener("keydown", async (event) => {
  if (event.ctrlKey && event.key.toLowerCase() === "s") {
    event.preventDefault();
    await saveLabels();
    return;
  }
  if (isTextEditingTarget(event.target)) {
    return;
  }
  if (event.ctrlKey && event.key.toLowerCase() === "z") {
    event.preventDefault();
    if (event.shiftKey) {
      redo();
    } else {
      undo();
    }
  } else if (event.ctrlKey && event.key.toLowerCase() === "y") {
    event.preventDefault();
    redo();
  } else if (event.key === "Delete") {
    deleteSelected();
  } else if (event.key === "Escape") {
    state.selected = null;
    setMode("select");
    renderAll();
  } else if (event.key === "ArrowUp" || event.key === "ArrowLeft") {
    event.preventDefault();
    await navigateBundle(-1);
  } else if (event.key === "ArrowDown" || event.key === "ArrowRight") {
    event.preventDefault();
    await navigateBundle(1);
  }
});

function updateDiagnostics() {
  if (!els.diagState) {
    return;
  }
  els.diagState.textContent = JSON.stringify({
    selectedPath: state.selectedBundle?.bundle_path || null,
    collapsedFolders: [...state.collapsedFolders].sort(),
    filter: els.searchBox.value,
    mode: state.mode,
    tab: state.tab,
    toggles: {
      labels: els.showLabels.checked,
      raw: els.showRaw.checked,
      diff: els.showDiff.checked,
      chars: els.showChars.checked,
      ignored: els.showIgnored.checked,
    },
    labelCount: state.spec?.detections.length || 0,
    ignoredCount: state.spec?.ignored.length || 0,
    rawCount: state.raw?.lines.length || 0,
    undoCount: state.undoStack.length,
    redoCount: state.redoStack.length,
    hasReport: Boolean(state.report),
    dirty: state.dirty,
    formDirty: state.formDirty,
    selected: state.selected,
    view: { ...state.view },
    canvas: { width: els.canvas.width, height: els.canvas.height },
    drawStats: { ...state.drawStats },
    status: els.statusLine.textContent,
  });
}

applyInitialUiState();
loadIndex().catch((error) => setStatus(error.message, true));
