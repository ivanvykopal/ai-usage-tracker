const panel = document.getElementById("panel");
const content = document.getElementById("content");
const titlebar = document.getElementById("titlebar");
const hideBtn = document.getElementById("hide-btn");
const usageLimitsEl = document.getElementById("usage-limits");

// Drag via the title bar using Tauri's native startDragging
titlebar.addEventListener("mousedown", () => {
  window.__TAURI__.window.getCurrent().window.startDragging();
});

hideBtn.addEventListener("click", () => {
  window.__TAURI__.core.invoke("toggle_visibility");
});

const compactBtn = document.getElementById("compact-btn");
let compactView = false;

compactBtn.addEventListener("click", () => {
  compactView = !compactView;
  window.__TAURI__.core.invoke("set_compact_view", { compact: compactView });
});

window.__TAURI__.event.listen("compact://update", (e) => {
  compactView = e.payload;
  panel.classList.toggle("compact", compactView);
});

const settingsBtn = document.getElementById("settings-btn");
const settingsPanel = document.getElementById("settings");
const opacitySlider = document.getElementById("opacity-slider");
const pollIntervalInput = document.getElementById("poll-interval-input");
const providerToggles = document.getElementById("provider-toggles");
const themeSelect = document.getElementById("theme-select");
const accentInput = document.getElementById("accent-input");

settingsBtn.addEventListener("click", async () => {
  const willOpen = settingsPanel.classList.contains("hidden");
  settingsPanel.classList.toggle("hidden");
  if (willOpen) await loadSettings();
});

async function loadSettings() {
  const [cfg, providers] = await Promise.all([
    window.__TAURI__.core.invoke("get_config"),
    window.__TAURI__.core.invoke("list_providers"),
  ]);
  opacitySlider.value = cfg.opacity;
  pollIntervalInput.value = cfg.poll_interval_ms;
  themeSelect.value = cfg.theme;
  accentInput.value = cfg.accent_color;
  providerToggles.innerHTML = providers.map(([key, label]) => `
    <label class="settings-row">
      <input type="checkbox" data-provider="${key}" ${cfg.enabled_agents.includes(key) ? "checked" : ""} />
      ${escapeHtml(label)}
    </label>
  `).join("");
  providerToggles.querySelectorAll("input[type=checkbox]").forEach(cb => {
    cb.addEventListener("change", onProviderToggleChange);
  });
}

function onProviderToggleChange() {
  const checked = Array.from(providerToggles.querySelectorAll("input[type=checkbox]:checked"))
    .map(cb => cb.dataset.provider);
  window.__TAURI__.core.invoke("set_enabled_agents", { agents: checked });
}

opacitySlider.addEventListener("input", () => {
  window.__TAURI__.core.invoke("set_opacity", { opacity: parseFloat(opacitySlider.value) });
});
pollIntervalInput.addEventListener("change", () => {
  window.__TAURI__.core.invoke("set_poll_interval", { ms: parseInt(pollIntervalInput.value, 10) });
});

window.__TAURI__.event.listen("opacity://update", (e) => {
  panel.style.opacity = e.payload;
  opacitySlider.value = e.payload;
});

window.__TAURI__.event.listen("theme://update", (e) => {
  const [theme, accent] = e.payload;
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.setProperty("--accent", accent);
});

function pushTheme() {
  window.__TAURI__.core.invoke("set_theme", { theme: themeSelect.value, accentColor: accentInput.value });
}
themeSelect.addEventListener("change", pushTheme);
accentInput.addEventListener("input", pushTheme);

// Subscribe to snapshot updates
const STATUS_LABEL = {
  waiting: "Waiting",
  thinking: "Thinking",
  executing: "Executing",
  done: "Done",
  unknown: "Unknown",
};

function fmt(n) {
  if (n >= 1000000) return (n / 1000000).toFixed(1) + "M";
  if (n >= 1000) return (n / 1000).toFixed(1) + "k";
  return String(n);
}

function fmtDuration(ms) {
  const mins = Math.floor(ms / 60000);
  if (mins < 1) return "<1m";
  if (mins < 60) return mins + "m";
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return m ? `${h}h${m}m` : `${h}h`;
}

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

function renderSnapshot(s) {
  if (!s.sessions || s.sessions.length === 0) {
    return '<div class="empty">No active AI assistants</div>';
  }
  function renderSessionRow(sess) {
    const bar = Math.min(100, Math.round(sess.context_percent || 0));
    const usage = (sess.total_input_tokens || 0)
      + (sess.total_output_tokens || 0)
      + (sess.total_cache_read || 0)
      + (sess.total_cache_create || 0);
    // started_at is 0 for agents that don't report a start time (e.g. codex
    // rollouts) — skip rate/duration in that case rather than showing a
    // meaningless huge elapsed time.
    const elapsedMs = sess.started_at > 0 ? Date.now() - sess.started_at : 0;
    const rate = elapsedMs > 1000 ? usage / (elapsedMs / 60000) : 0;
    const usageRow = elapsedMs > 1000
      ? `<span>&#931;${fmt(usage)} / ${fmtDuration(elapsedMs)}</span><span>${fmt(rate)} tok/min</span>`
      : `<span>&#931;${fmt(usage)}</span>`;
    return `
      <div class="row${sess.stalled ? " stalled" : ""}">
        <div class="head">
          <span class="dot dot-${sess.agent_cli}"></span>
          <span class="agent">${sess.agent_cli}</span>
          ${sess.model ? `<span class="model">${escapeHtml(sess.model)}</span>` : ""}
          <span class="status status-${sess.status}">${STATUS_LABEL[sess.status] || sess.status}</span>
        </div>
        <div class="bar"><div class="bar-fill" style="width:${bar}%"></div></div>
        <div class="meta">
          <span>&#8595;${fmt(sess.total_input_tokens || 0)}</span>
          <span>&#8593;${fmt(sess.total_output_tokens || 0)}</span>
          ${usageRow}
          <span>ctx ${bar}%</span>
          ${sess.cost_usd != null ? `<span class="cost">$${sess.cost_usd.toFixed(3)}</span>` : ""}
          <span>${sess.mem_mb || 0}MB</span>
          <span class="task">${escapeHtml(sess.current_task || "")}</span>
        </div>
      </div>`;
  }

  function groupByProject(sessions) {
    const groups = new Map();
    for (const sess of sessions) {
      const key = sess.project_name || "(unknown)";
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(sess);
    }
    return groups;
  }

  const groups = groupByProject(s.sessions);
  const rows = Array.from(groups.entries()).map(([project, sessions]) => `
    <div class="project-group">
      <div class="project-header">${escapeHtml(project)} <span class="project-count">${sessions.length}</span></div>
      ${sessions.map(renderSessionRow).join("")}
    </div>
  `).join("");
  const totalTokens = s.total_tokens || 0;
  const totalCost = s.total_cost_usd || 0;
  const liveCount = s.sessions.length;
  return `<div class="rows">${rows}</div>
          <div class="footer">total ${fmt(totalTokens)} tok &#183; $${totalCost.toFixed(2)} &#183; ${liveCount} live</div>`;
}

const AGENT_LABEL = { claude: "Claude", codex: "Codex", hermes: "Hermes" };

function fmtCountdown(resetsAtSecs) {
  if (resetsAtSecs == null) return null;
  const ms = resetsAtSecs * 1000 - Date.now();
  if (ms <= 0) return "now";
  const mins = Math.floor(ms / 60000);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) {
    const remMins = mins % 60;
    return remMins ? `${hours}h${remMins}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

function renderUsageWindow(label, pct, resetsAt, etaMs) {
  // A provider only reports the windows its plan actually has (e.g. Codex's
  // free plan reports only "month", no five-hour/weekly at all) — omit
  // entirely rather than showing a "label —" placeholder for windows that
  // were never reported.
  if (pct == null) return "";
  const clamped = Math.min(100, Math.max(0, Math.round(pct)));
  const countdown = fmtCountdown(resetsAt);
  const eta = etaMs != null ? fmtDuration(etaMs) : null;
  return `<span class="usage-window">
      ${label} <span class="usage-pct">${clamped}%</span>
      <span class="bar usage-bar"><span class="bar-fill" style="width:${clamped}%"></span></span>
      ${countdown ? `<span class="usage-reset">resets ${countdown}</span>` : ""}
      ${eta ? `<span class="usage-eta">&#8776;${eta} to limit</span>` : ""}
    </span>`;
}

function renderUsageLimits(usageLimits) {
  const agents = Object.keys(usageLimits || {});
  if (agents.length === 0) return "";
  const rows = agents.map(agent => {
    const rl = usageLimits[agent];
    const label = AGENT_LABEL[agent] || agent;
    const windows = [
      renderUsageWindow("5h", rl.five_hour_pct, rl.five_hour_resets_at, rl.five_hour_eta_ms),
      renderUsageWindow("week", rl.seven_day_pct, rl.seven_day_resets_at, rl.seven_day_eta_ms),
      renderUsageWindow("month", rl.monthly_pct, rl.monthly_resets_at, rl.monthly_eta_ms),
    ].filter(Boolean).join("");
    if (!windows) return "";
    return `<div class="usage-row">
        <span class="usage-agent">${escapeHtml(label)}</span>
        ${windows}
      </div>`;
  }).join("");
  return rows;
}

window.__TAURI__.event.listen("snapshot://update", (e) => {
  usageLimitsEl.innerHTML = renderUsageLimits(e.payload.usage_limits);
  content.innerHTML = renderSnapshot(e.payload);
});

const historyCanvas = document.getElementById("history-chart");
const historyCtx = historyCanvas.getContext("2d");

function drawSparkline(points) {
  const w = historyCanvas.width = historyCanvas.clientWidth;
  const h = historyCanvas.height;
  historyCtx.clearRect(0, 0, w, h);
  if (points.length < 2) return;
  const values = points.map(p => p[1]);
  const min = Math.min(...values);
  const max = Math.max(...values, min + 1);
  historyCtx.strokeStyle = "#6aa0ff";
  historyCtx.lineWidth = 1.5;
  historyCtx.beginPath();
  points.forEach((p, i) => {
    const x = (i / (points.length - 1)) * w;
    const y = h - ((p[1] - min) / (max - min)) * h;
    if (i === 0) historyCtx.moveTo(x, y);
    else historyCtx.lineTo(x, y);
  });
  historyCtx.stroke();
}

async function refreshHistory() {
  try {
    const points = await window.__TAURI__.core.invoke("get_usage_history", { agent: "claude", hours: 6 });
    drawSparkline(points);
  } catch (e) {
    // history disabled or db unavailable — leave the canvas blank.
  }
}
refreshHistory();
setInterval(refreshHistory, 60000);
