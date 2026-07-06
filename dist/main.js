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
  const rows = s.sessions.map(sess => {
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
      <div class="row">
        <div class="head">
          <span class="dot dot-${sess.agent_cli}"></span>
          <span class="agent">${sess.agent_cli}</span>
          ${sess.model ? `<span class="model">${escapeHtml(sess.model)}</span>` : ""}
          <span class="proj">${escapeHtml(sess.project_name || "")}</span>
          <span class="status status-${sess.status}">${STATUS_LABEL[sess.status] || sess.status}</span>
        </div>
        <div class="bar"><div class="bar-fill" style="width:${bar}%"></div></div>
        <div class="meta">
          <span>&#8595;${fmt(sess.total_input_tokens || 0)}</span>
          <span>&#8593;${fmt(sess.total_output_tokens || 0)}</span>
          ${usageRow}
          <span>ctx ${bar}%</span>
          <span>${sess.mem_mb || 0}MB</span>
          <span class="task">${escapeHtml(sess.current_task || "")}</span>
        </div>
      </div>`;
  }).join("");
  const totalTokens = s.total_tokens || 0;
  const liveCount = s.sessions.length;
  return `<div class="rows">${rows}</div>
          <div class="footer">total ${fmt(totalTokens)} tok &#183; ${liveCount} live</div>`;
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

function renderUsageWindow(label, pct, resetsAt) {
  if (pct == null) return `<span>${label} —</span>`;
  const clamped = Math.min(100, Math.max(0, Math.round(pct)));
  const countdown = fmtCountdown(resetsAt);
  return `<span class="usage-window">
      ${label} <span class="usage-pct">${clamped}%</span>
      <span class="bar usage-bar"><span class="bar-fill" style="width:${clamped}%"></span></span>
      ${countdown ? `<span class="usage-reset">resets ${countdown}</span>` : ""}
    </span>`;
}

function renderUsageLimits(usageLimits) {
  const agents = Object.keys(usageLimits || {});
  if (agents.length === 0) return "";
  const rows = agents.map(agent => {
    const rl = usageLimits[agent];
    const label = AGENT_LABEL[agent] || agent;
    return `<div class="usage-row">
        <span class="usage-agent">${escapeHtml(label)}</span>
        ${renderUsageWindow("5h", rl.five_hour_pct, rl.five_hour_resets_at)}
        ${renderUsageWindow("week", rl.seven_day_pct, rl.seven_day_resets_at)}
      </div>`;
  }).join("");
  return rows;
}

window.__TAURI__.event.listen("snapshot://update", (e) => {
  usageLimitsEl.innerHTML = renderUsageLimits(e.payload.usage_limits);
  content.innerHTML = renderSnapshot(e.payload);
});
