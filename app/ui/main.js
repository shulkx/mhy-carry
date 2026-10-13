"use strict";
// MhyCarry 界面：只按 Rust 端推送的状态快照显示（设计文档第 6.3 节），自己不记业务状态。
// 外部文本（窗口标题、服务器返回的文字）一律用 textContent，不用 innerHTML。

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const TARGETS = ["auto", "window", "monitor"];
const CREDENTIAL_LABELS = { unchecked: "凭证未检查", valid: "凭证有效", invalid: "凭证已失效" };
const BUSY_BIND = ["requesting", "waiting_scan", "waiting_confirm"];
const WAITING = ["checking_credential", "waiting_for_window", "paused", "game_in_background", "awaiting_confirm"];
const BAD_STOPS = ["credential_invalid", "target_gone", "capture_failing", "credential_check_failed"];

let state = null;
let target = null; // 界面上选的截图目标；还没选过时用设置里的默认值
let targetList = [];

function el(tag, attrs = {}, text) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, value);
  if (text !== undefined) node.textContent = text;
  return node;
}

function button(label, className, onClick) {
  const node = el("button", { type: "button", class: className }, label);
  node.addEventListener("click", onClick);
  return node;
}

function showError(error) {
  $("error").textContent = String(error);
  $("error").hidden = false;
}

async function call(command, args) {
  $("error").hidden = true;
  try {
    return await invoke(command, args);
  } catch (error) {
    showError(error);
    return null;
  }
}

function currentTarget() {
  return target ?? state.settings.default_target;
}

// ---- 账号和绑定 ----

function renderAccount() {
  const box = $("account");
  box.replaceChildren();
  const { account, bind } = state;
  if (bind.kind !== "idle" && bind.kind !== "succeeded") {
    renderBind(box, bind);
    return;
  }
  if (account.kind === "none") {
    box.append(el("div", { class: "meta" }, "还没有绑定账号。用米游社 App 扫码绑定后，就能用这个账号确认游戏登录。"));
    if (account.notice) box.append(el("div", { class: "message bad" }, account.notice));
    const actions = el("div", { class: "actions" });
    actions.append(button("用米游社扫码绑定", "btn primary", () => call("bind_start")));
    // 账号文件损坏时留下的备份里可能有明文凭证，未绑定时也要能删掉。
    if (state.leftovers > 0) actions.append(button("删除旧账号文件", "btn danger", () => call("unbind")));
    box.append(actions);
    return;
  }
  if (account.kind === "unavailable") {
    box.append(el("div", { class: "message bad" }, account.message));
    return;
  }
  const row = el("div", { class: "account-row" });
  row.append(el("span", { class: "uid" }, account.uid),
    el("span", { class: "badge " + account.credential }, CREDENTIAL_LABELS[account.credential]));
  box.append(row, el("div", { class: "meta" }, "绑定于 " + account.bound_at));
  // 几秒后由 Rust 端自动收起。
  if (bind.kind === "succeeded") box.append(el("div", { class: "message ok" }, "绑定成功。"));
  if (account.credential === "invalid") {
    box.append(el("div", { class: "message bad" }, "这个账号的登录状态已失效，重新绑定后才能继续使用。"));
  }
  const scanning = state.scan.kind !== "idle";
  const actions = el("div", { class: "actions" });
  const rebind = button("重新绑定", account.credential === "invalid" ? "btn primary" : "btn", () => call("bind_start"));
  const unbind = button("解绑", "btn danger", () => call("unbind"));
  rebind.disabled = scanning;
  unbind.disabled = scanning;
  actions.append(rebind, unbind);
  box.append(actions);
}

function renderBind(box, bind) {
  const close = () => call("bind_cancel");
  const actions = el("div", { class: "actions" });
  if (bind.kind === "waiting_scan" || bind.kind === "waiting_confirm") {
    const wrap = el("div", { class: "bind" });
    wrap.append(el("img", { src: bind.qr, alt: "绑定用的二维码" }));
    const side = el("div");
    const [step, hint] = bind.kind === "waiting_scan"
      ? ["用米游社 App 扫描左边的二维码", "扫码后在手机上确认登录。"]
      : ["已扫码，请在手机上确认", "确认后自动保存账号。"];
    side.append(el("div", { class: "step" }, step), el("div", { class: "meta" }, hint));
    actions.append(button("取消", "btn", close));
    side.append(actions);
    wrap.append(side);
    box.append(wrap);
    return;
  }
  const texts = {
    requesting: ["正在获取二维码…", "message"],
    expired: ["二维码已过期。", "message bad"],
    cancelled: ["已在手机上取消。", "message bad"],
    failed: ["绑定失败：" + (bind.message ?? ""), "message bad"],
  };
  const [text, className] = texts[bind.kind];
  box.append(el("div", { class: className }, text));
  if (bind.kind === "requesting") {
    actions.append(button("取消", "btn", close));
  } else {
    actions.append(button(bind.kind === "expired" ? "刷新" : "重新绑定", "btn primary", () => call("bind_start")),
      button("关闭", "btn", close));
  }
  box.append(actions);
}

// ---- 扫描 ----

function dotClass(scan) {
  if (scan.kind === "scanning" || scan.kind === "logging_in") return "scanning";
  if (WAITING.includes(scan.kind)) return "waiting";
  if (scan.kind === "idle" && scan.last) {
    if (scan.last.kind === "logged_in") return "ok";
    if (BAD_STOPS.includes(scan.last.kind)) return "bad";
  }
  return "";
}

async function loadTargets(kind) {
  // 先记下原来的选择再清空列表，否则刷新后就丢了。
  const keep = $("target-select").value;
  targetList = [];
  fillTargets(keep);
  if (kind === "auto") return;
  targetList = (await call("list_targets", { kind })) ?? [];
  fillTargets(keep);
}

function fillTargets(keep) {
  const select = $("target-select");
  select.replaceChildren(...targetList.map((item) => el("option", { value: String(item.id) }, item.label)));
  // 刷新后保留原来的选择；没选过时选游戏窗口或主显示器。
  const preferred = targetList.find((item) => item.preferred);
  if (targetList.some((item) => String(item.id) === keep)) select.value = keep;
  else if (preferred) select.value = String(preferred.id);
}

function renderScan() {
  const scan = state.scan;
  const idle = scan.kind === "idle";
  const chosen = currentTarget();
  for (const name of TARGETS) {
    const node = $("target-" + name);
    node.setAttribute("aria-pressed", String(name === chosen));
    node.disabled = !idle;
  }
  $("target-pick").hidden = chosen === "auto";
  $("target-select").disabled = !idle;
  $("target-refresh").disabled = !idle;

  const start = $("scan-button");
  if (idle) {
    const ready = state.account.kind === "bound" && state.account.credential !== "invalid"
      && !BUSY_BIND.includes(state.bind.kind);
    start.textContent = "开始扫描";
    start.disabled = !ready;
  } else {
    start.textContent = "停止";
    // 登录请求已经发出，要等结果出来。
    start.disabled = scan.kind === "logging_in";
  }

  $("status-dot").className = "dot " + dotClass(scan);
  $("status-text").textContent = state.scan_text;
  const notice = $("status-notice");
  notice.replaceChildren();
  if (state.notice_text) {
    notice.append(el("time", {}, state.notice_at ?? ""), "提示：" + state.notice_text);
  } else if (idle && state.account.kind !== "bound") {
    notice.textContent = "绑定账号后才能开始扫描。";
  }
}

// ---- 设置和页脚 ----

function renderSettings() {
  const select = $("setting-target");
  const input = $("setting-timeout");
  if (document.activeElement !== select) select.value = state.settings.default_target;
  if (document.activeElement !== input) input.value = String(state.settings.idle_timeout_secs);
}

function saveSettings() {
  call("update_settings", {
    default_target: $("setting-target").value,
    idle_timeout_secs: Math.max(30, Math.round(Number($("setting-timeout").value)) || 300),
  });
}

function renderFooter() {
  const footer = $("footer");
  footer.replaceChildren("凭证明文保存在 ", el("code", {}, state.credential_file), "，解绑会删除它。");
  if (state.leftovers > 0) {
    const how = state.account.kind === "bound" ? "解绑时一并删除。" : "可以点上面的“删除旧账号文件”删掉。";
    footer.append(el("div", {}, "另有 " + state.leftovers + " 个旧账号文件（账号文件损坏时留下的备份），" + how));
  }
}

function render(next) {
  // 推送可能乱序到达，丢掉比手上旧的快照。
  if (state && next.seq < state.seq) return;
  state = next;
  renderAccount();
  renderScan();
  renderSettings();
  renderFooter();
}

// ---- 事件 ----

for (const name of TARGETS) {
  $("target-" + name).addEventListener("click", () => {
    target = name;
    renderScan();
    loadTargets(name);
  });
}
$("target-refresh").addEventListener("click", () => loadTargets(currentTarget()));
$("scan-button").addEventListener("click", () => {
  if (state.scan.kind !== "idle") {
    call("scan_stop");
    return;
  }
  const chosen = currentTarget();
  const id = chosen === "auto" ? null : Number($("target-select").value) || null;
  call("scan_start", { target: chosen, id });
});
$("setting-target").addEventListener("change", saveSettings);
$("setting-timeout").addEventListener("change", saveSettings);

async function main() {
  // 先开始监听再取快照，中间推送的变化不会漏掉。
  await listen("state-changed", (event) => render(event.payload));
  render(await invoke("get_state"));
  loadTargets(currentTarget());
}

main().catch(showError);
