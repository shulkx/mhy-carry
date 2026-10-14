"use strict";
// MhyCarry 界面：只按 Rust 端推送的状态快照显示（设计文档第 6.3 节），自己不记业务状态。
// 外部文本（窗口标题、服务器返回的文字、昵称和备注）一律用 textContent，不用 innerHTML。

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const TARGETS = ["auto", "window", "monitor"];
const CREDENTIAL_LABELS = { unchecked: "凭证未检查", valid: "凭证有效", invalid: "凭证已失效" };
const SHORT_CREDENTIAL = { unchecked: "未检查", valid: "有效", invalid: "已失效" };
const BUSY_BIND = ["requesting", "waiting_scan", "waiting_confirm"];
const WAITING = ["checking_credential", "waiting_for_window", "paused", "game_in_background", "awaiting_confirm"];
const BAD_STOPS = ["credential_invalid", "target_gone", "capture_failing", "credential_check_failed"];
const MAX_NOTE_CHARS = 20;

let state = null;
let target = null; // 界面上选的截图目标；还没选过时用设置里的默认值
let targetList = [];
// 只影响显示的界面状态：打开的弹出层（"switch" 切换账号、"menu" 卡片菜单）、是否在改备注、
// 正在重新绑定的通行证 ID（只用来显示提示）。
let open = null;
let editing = false;
let rebindFor = null;

function el(tag, attrs = {}, text) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, value);
  if (text !== undefined) node.textContent = text;
  return node;
}

function button(label, className, onClick, attrs = {}) {
  const node = el("button", { type: "button", class: className, ...attrs }, label);
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

// ---- 账号 ----

function accountList() {
  return state.accounts.kind === "list" ? state.accounts.items : [];
}

function activeAccount() {
  return accountList().find((item) => item.id === state.accounts.active) ?? null;
}

// 扫描中、绑定中不能动账号（设计文档第 9 节）。
function accountsLocked() {
  return state.scan.kind !== "idle" || BUSY_BIND.includes(state.bind.kind);
}

function startBind(passportId = null) {
  rebindFor = passportId;
  open = null;
  editing = false;
  call("bind_start");
}

function avatar(item, small = false) {
  // 没有备注也没查到昵称时，名称就是通行证 ID，头像显示 #。
  const initial = item.note || item.role ? Array.from(item.name)[0] : "#";
  const classes = "avatar" + (small ? " small" : "") + (item.credential === "invalid" ? " invalid" : "");
  return el("span", { class: classes, "aria-hidden": "true" }, initial);
}

function level(item) {
  return el("span", { class: "level" }, "Lv." + item.role.level);
}

function renderAccountHead() {
  const list = accountList();
  const locked = accountsLocked();
  const toggle = $("switch-account");
  toggle.hidden = list.length < 2;
  toggle.disabled = locked;
  toggle.setAttribute("aria-expanded", String(open === "switch"));
  toggle.replaceChildren("切换账号 ", el("span", { class: "count" }, "(" + list.length + ")"),
    el("span", { class: "caret", "aria-hidden": "true" }, "▼"));
  const add = $("add-account");
  add.hidden = state.accounts.kind !== "list";
  add.disabled = locked;
}

function renderAccount() {
  const box = $("account");
  box.replaceChildren();
  const { accounts, bind } = state;
  if (accountsLocked() || accounts.kind !== "list") {
    open = null;
    editing = false;
  }
  if (bind.kind === "idle" || bind.kind === "succeeded") rebindFor = null;
  renderAccountHead();

  if (bind.kind !== "idle" && bind.kind !== "succeeded") {
    renderBind(box, bind);
  } else if (accounts.kind === "unavailable") {
    box.append(el("div", { class: "message bad" }, accounts.message));
  } else if (accounts.kind === "none") {
    box.append(el("div", { class: "meta" }, "还没有绑定账号。用米游社 App 扫码绑定后，就能用这个账号确认游戏登录。"));
    if (accounts.notice) box.append(el("div", { class: "message bad" }, accounts.notice));
    const actions = el("div", { class: "actions" });
    actions.append(button("用米游社扫码绑定", "btn primary", () => startBind()));
    box.append(actions);
  } else {
    box.append(renderCard(activeAccount()));
  }

  if (open === "switch" && accountList().length > 1) box.append(renderSwitcher());
  if (bind.kind === "succeeded") {
    // 几秒后由 Rust 端自动收起。
    box.append(el("div", { class: "message ok" },
      bind.added ? "已添加账号，并设为使用中。" : "已更新这个账号的凭证，并设为使用中。"));
  }
  if (state.leftovers > 0) {
    // 账号文件损坏时留下的备份里可能有明文凭证，单独删除（设计文档第 6.4 节）。
    const actions = el("div", { class: "actions" });
    const remove = button("删除旧账号文件", "btn danger", () => call("delete_leftovers"));
    remove.disabled = state.scan.kind !== "idle";
    actions.append(remove);
    box.append(actions);
  }
}

function renderCard(item) {
  const card = el("div", { class: "card" + (item.credential === "invalid" ? " invalid" : "") });
  const main = el("div", { class: "card-main" });
  if (editing) {
    main.append(renderNoteEditor(item));
  } else {
    const title = el("div", { class: "card-title" });
    title.append(el("span", { class: "name" }, item.name));
    if (item.role) title.append(level(item));
    main.append(title);
    // 名称用的是备注时，下面补一行原神昵称。
    if (item.note && item.role) main.append(el("div", { class: "card-sub" }, "原神昵称 " + item.role.nickname));
  }

  const chips = el("div", { class: "chips" });
  for (const [label, value] of [["原神 UID", item.role?.game_uid], ["通行证", item.passport_id]]) {
    const chip = el("span", { class: "chip" });
    chip.append(el("span", { class: "k" }, label),
      el("span", { class: "v" + (value ? "" : " missing") }, value ?? "未查到"));
    chips.append(chip);
  }

  const side = el("div", { class: "card-side" });
  const more = button("⋯", "more", () => {
    open = open === "menu" ? null : "menu";
    renderAccount();
  }, { "aria-label": "更多操作", "aria-haspopup": "menu", "aria-expanded": String(open === "menu") });
  more.disabled = accountsLocked();
  side.append(el("span", { class: "cred " + item.credential }, CREDENTIAL_LABELS[item.credential]), more);

  card.append(avatar(item), main, side, chips);
  if (open === "menu") card.append(renderMenu(item));
  return card;
}

function renderNoteEditor(item) {
  const box = el("div", { class: "name-edit" });
  const input = el("input", {
    type: "text", id: "note-input", maxlength: String(MAX_NOTE_CHARS),
    "aria-label": "备注", placeholder: "备注，比如“大号”",
  });
  input.value = item.note;
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      editing = false;
      call("set_note", { id: item.id, note: input.value });
      renderAccount();
    } else if (event.key === "Escape") {
      editing = false;
      renderAccount();
    }
  });
  box.append(input, el("div", { class: "hint" }, "回车保存，Esc 取消；最多 " + MAX_NOTE_CHARS + " 个字"));
  queueMicrotask(() => input.focus());
  return box;
}

function renderMenu(item) {
  const menu = el("div", { class: "popover menu", role: "menu" });
  const items = [
    button("编辑备注", "", () => {
      open = null;
      editing = true;
      renderAccount();
    }, { role: "menuitem" }),
    button("重新绑定", "", () => startBind(item.passport_id), { role: "menuitem" }),
    button("解绑", "danger", () => {
      open = null;
      renderAccount();
      call("unbind", { id: item.id });
    }, { role: "menuitem" }),
  ];
  // 凭证失效的账号最常用的是重新绑定，排到第一项。
  if (item.credential === "invalid") items.unshift(items.splice(1, 1)[0]);
  menu.append(...items);
  return menu;
}

function renderSwitcher() {
  const list = accountList();
  const box = el("div", { class: "popover switcher" });
  const rows = el("div", { class: "switcher-list", role: "listbox", "aria-label": "切换使用中的账号" });
  for (const item of list) {
    const selected = item.id === state.accounts.active;
    const row = button("", "switch-row", () => {
      open = null;
      if (!selected) call("set_active", { id: item.id });
      renderAccount();
    }, { role: "option", "aria-selected": String(selected) });
    const title = el("span", { class: "row-title" });
    title.append(el("span", {}, item.name));
    if (item.role) title.append(level(item));
    row.append(el("span", { class: "check", "aria-hidden": "true" }, selected ? "✓" : ""), avatar(item, true), title,
      el("span", { class: "cred " + item.credential }, SHORT_CREDENTIAL[item.credential]),
      el("span", { class: "row-id" }, item.role ? "原神 UID " + item.role.game_uid : "通行证 " + item.passport_id));
    rows.append(row);
  }
  box.append(rows, el("div", { class: "switcher-foot" },
    "共 " + list.length + " 个账号。要编辑或解绑某个账号，先切换到它，再用卡片上的“⋯”。"));
  return box;
}

function renderBind(box, bind) {
  const close = () => call("bind_cancel");
  const actions = el("div", { class: "actions" });
  if (bind.kind === "waiting_scan" || bind.kind === "waiting_confirm") {
    const wrap = el("div", { class: "bind" });
    wrap.append(el("img", { src: bind.qr, alt: "绑定用的二维码" }));
    const side = el("div");
    let [step, hint] = bind.kind === "waiting_scan"
      ? ["用米游社 App 扫描左边的二维码", "扫码时登录的是哪个账号，就添加哪个。扫码后在手机上确认。"]
      : ["已扫码，请在手机上确认", "确认后自动保存账号。"];
    if (rebindFor && bind.kind === "waiting_scan") hint = "请先在米游社 App 里登录通行证 " + rebindFor + "，再扫码。";
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
    actions.append(button(bind.kind === "expired" ? "刷新" : "重新绑定", "btn primary", () => startBind(rebindFor)),
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
  const active = activeAccount();
  if (idle) {
    // 大按钮写着扫描用的账号（设计文档第 6.3 节）。
    const ready = active && active.credential !== "invalid" && !BUSY_BIND.includes(state.bind.kind);
    start.textContent = active ? "开始扫描（" + active.name + "）" : "开始扫描";
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
  } else if (idle && state.accounts.kind !== "list") {
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
  footer.replaceChildren("凭证明文保存在 ", el("code", {}, state.credential_file), "，解绑某个账号会删除它的凭证。");
  if (state.leftovers > 0) {
    footer.append(el("div", {}, "另有 " + state.leftovers + " 个旧账号文件（账号文件损坏时留下的备份），可以点上面的“删除旧账号文件”删掉。"));
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
$("switch-account").addEventListener("click", () => {
  open = open === "switch" ? null : "switch";
  editing = false;
  renderAccount();
});
$("add-account").addEventListener("click", () => startBind());
// Esc 或点别处关掉弹出层。
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && open) {
    open = null;
    renderAccount();
  }
});
document.addEventListener("click", (event) => {
  if (open && !event.target.closest(".popover, #switch-account, .more")) {
    open = null;
    renderAccount();
  }
});

async function main() {
  // 先开始监听再取快照，中间推送的变化不会漏掉。
  await listen("state-changed", (event) => render(event.payload));
  render(await invoke("get_state"));
  loadTargets(currentTarget());
}

main().catch(showError);
