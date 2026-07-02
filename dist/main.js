const panel = document.getElementById("panel");
const content = document.getElementById("content");
const titlebar = document.getElementById("titlebar");
const hideBtn = document.getElementById("hide-btn");
const opacitySlider = document.getElementById("opacity");

// Drag via the title bar using Tauri's native startDragging
titlebar.addEventListener("mousedown", () => {
  window.__TAURI__.window.getCurrent().window.startDragging();
});

hideBtn.addEventListener("click", () => {
  window.__TAURI__.core.invoke("toggle_visibility");
});

// Opacity slider — applied as CSS on the panel (Tauri has no cross-platform
// native window-opacity API), persisted to config via the backend command.
function applyOpacity(value) {
  panel.style.opacity = value;
  opacitySlider.value = value;
}

opacitySlider.addEventListener("input", () => {
  const value = parseFloat(opacitySlider.value);
  applyOpacity(value);
  window.__TAURI__.core.invoke("set_opacity", { opacity: value });
});

window.__TAURI__.event.listen("opacity://update", (e) => {
  applyOpacity(e.payload);
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
  if (n >= 1000) return (n / 1000).toFixed(1) + "k";
  return String(n);
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
    return `
      <div class="row">
        <div class="head">
          <span class="dot dot-${sess.agent_cli}"></span>
          <span class="agent">${sess.agent_cli}</span>
          <span class="proj">${escapeHtml(sess.project_name || "")}</span>
          <span class="status status-${sess.status}">${STATUS_LABEL[sess.status] || sess.status}</span>
        </div>
        <div class="bar"><div class="bar-fill" style="width:${bar}%"></div></div>
        <div class="meta">
          <span>&#8595;${fmt(sess.total_input_tokens || 0)}</span>
          <span>&#8593;${fmt(sess.total_output_tokens || 0)}</span>
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
