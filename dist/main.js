const panel = document.getElementById("panel");
const content = document.getElementById("content");
const titlebar = document.getElementById("titlebar");
const hideBtn = document.getElementById("hide-btn");

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
    const rl = sess.rate_limit;
    const rateLimitRow = rl
      ? `<div class="rate-limit">
          ${rl.five_hour_pct != null ? `<span>5h ${Math.round(rl.five_hour_pct)}%</span>` : ""}
          ${rl.seven_day_pct != null ? `<span>week ${Math.round(rl.seven_day_pct)}%</span>` : ""}
        </div>`
      : "";
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
        ${rateLimitRow}
      </div>`;
  }).join("");
  const totalTokens = s.total_tokens || 0;
  const liveCount = s.sessions.length;
  return `<div class="rows">${rows}</div>
          <div class="footer">total ${fmt(totalTokens)} tok &#183; ${liveCount} live</div>`;
}

window.__TAURI__.event.listen("snapshot://update", (e) => {
  content.innerHTML = renderSnapshot(e.payload);
});
