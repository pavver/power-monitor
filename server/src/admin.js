"use strict";
/* The Operator controls below (Control, ripple, actionButton, textField, say)
   are the device panel's, firmware/ui/index.html; keep the two in step. */
const $ = id => document.getElementById(id);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const POLL_MS = 5000;

function el(tag, attrs, ...children) {
  const node = document.createElement(tag);
  Object.entries(attrs || {}).forEach(([k, v]) => k === "class" ? node.className = v : node.setAttribute(k, v));
  children.forEach(c => node.append(c));
  return node;
}

/* ---------- OperatorIcons (Material paths, 24×24) ---------- */
const ICONS = {
  check: "M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z",
  close: "M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
  eye: "M12 4.5C7 4.5 2.73 7.61 1 12c1.73 4.39 6 7.5 11 7.5s9.27-3.11 11-7.5c-1.73-4.39-6-7.5-11-7.5zM12 17c-2.76 0-5-2.24-5-5s2.24-5 5-5 5 2.24 5 5-2.24 5-5 5zm0-8c-1.66 0-3 1.34-3 3s1.34 3 3 3 3-1.34 3-3-1.34-3-3-3z",
  eyeOff: "M12 7c2.76 0 5 2.24 5 5 0 .65-.13 1.26-.36 1.83l2.92 2.92c1.51-1.26 2.7-2.89 3.43-4.75-1.73-4.39-6-7.5-11-7.5-1.4 0-2.74.25-3.98.7l2.16 2.16C10.74 7.13 11.35 7 12 7zM2 4.27l2.28 2.28.46.46C3.08 8.3 1.78 10.02 1 12c1.73 4.39 6 7.5 11 7.5 1.55 0 3.03-.3 4.38-.84l.42.42L19.73 22 21 20.73 3.27 3 2 4.27zM7.53 9.8l1.55 1.55c-.05.21-.08.43-.08.65 0 1.66 1.34 3 3 3 .22 0 .44-.03.65-.08l1.55 1.55c-.67.33-1.41.53-2.2.53-2.76 0-5-2.24-5-5 0-.79.2-1.53.53-2.2zm4.31-.78l3.15 3.15.02-.16c0-1.66-1.34-3-3-3l-.17.01z",
};
const SVG = "http://www.w3.org/2000/svg";
function icon(name) {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", "ic");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(SVG, "path");
  path.setAttribute("d", ICONS[name]);
  svg.append(path);
  return svg;
}

/* ---------- ControlState frame (operatorStateBorder) ----------
   Strokes are inset by half their width so they stay inside the control.
   The pending segment is 25% of the perimeter and runs clockwise in 2 s. */
const frames = new ResizeObserver(entries => entries.forEach(e => layoutFrame(e.target)));
function layoutFrame(node) {
  const svg = node.querySelector(":scope>.sf");
  const w = node.offsetWidth, h = node.offsetHeight;
  if (!svg || !w || !h) return;
  for (const rect of svg.children) {
    const inset = (+rect.dataset.stroke || 0) / 2;
    rect.setAttribute("x", inset); rect.setAttribute("y", inset);
    rect.setAttribute("width", Math.max(0, w - 2 * inset)); rect.setAttribute("height", Math.max(0, h - 2 * inset));
  }
  const perimeter = 2 * (w + h - 2 * 1.5);
  const run = svg.querySelector(".run");
  run.style.setProperty("--perimeter", perimeter + "px");
  run.setAttribute("stroke-dasharray", perimeter / 4 + " " + perimeter * 3 / 4);
}

class Control {
  constructor(node) {
    this.node = node;
    this.state = "idle";
    node.classList.add("ctl");
    const svg = document.createElementNS(SVG, "svg");
    svg.setAttribute("class", "sf");
    svg.setAttribute("aria-hidden", "true");
    for (const [cls, stroke] of [["tint", 0], ["base", 1], ["run", 1.5], ["stat", 2]]) {
      const rect = document.createElementNS(SVG, "rect");
      rect.setAttribute("class", cls);
      rect.dataset.stroke = stroke;
      svg.append(rect);
    }
    node.prepend(svg);
    node.dataset.state = "idle";
    frames.observe(node);
    ripple(node);
  }
  /* idle | pending | ok | err. Leaving ok/err fades the status colour over 500 ms. */
  set(next) {
    const node = this.node, was = this.state;
    if (next === was) return;
    this.state = next;
    clearTimeout(this.exitTimer);
    if (next === "ok" || next === "err") {
      node.classList.remove("exit");
      node.dataset.tone = next;
    } else if (was === "ok" || was === "err") {
      node.classList.add("exit");
      this.exitTimer = setTimeout(() => { node.classList.remove("exit"); delete node.dataset.tone; }, 500);
    }
    node.dataset.state = next;
  }
}

/* Flat ripple from the touch point; Pressed state while held. */
function ripple(node) {
  if (node.tagName !== "BUTTON") return;
  node.addEventListener("pointerdown", e => {
    if (node.disabled || node.dataset.state === "pending") return;
    const box = node.getBoundingClientRect();
    const r = Math.hypot(Math.max(e.clientX - box.left, box.right - e.clientX), Math.max(e.clientY - box.top, box.bottom - e.clientY));
    const dot = el("span", {class: "rip"});
    Object.assign(dot.style, {width: 2 * r + "px", height: 2 * r + "px", left: e.clientX - box.left - r + "px", top: e.clientY - box.top - r + "px"});
    node.append(dot);
    node.classList.add("pressed");
    const release = () => {
      node.classList.remove("pressed");
      dot.classList.add("out");
      setTimeout(() => dot.remove(), 300);
      ["pointerup", "pointercancel", "pointerleave"].forEach(t => node.removeEventListener(t, release));
    };
    ["pointerup", "pointercancel", "pointerleave"].forEach(t => node.addEventListener(t, release));
  });
}

/* ---------- OperatorActionButton ----------
   Pending: label hidden (it keeps the width), spinner, running border.
   Result: 0.5 s icon, 0.3 s label in the result colour, then 0.5 s fade. */
function actionButton(node, work, options = {}) {
  const label = el("span", {class: "lbl"});
  label.append(...node.childNodes);
  const control = new Control(node);
  node.append(label,
    el("span", {class: "over wait"}, el("span", {class: "spin"})),
    el("span", {class: "over ok"}, icon("check")),
    el("span", {class: "over err"}, icon("close")));
  let run = 0;
  control.run = async () => {
    if (control.state === "pending" || node.disabled) return;
    if (options.danger && control.state !== "idle") return;
    const id = ++run;
    control.set("pending");
    options.frame?.set("pending");
    let ok = true;
    try { await work(); } catch (err) {
      ok = false;
      if (options.msg) say(options.msg(), err.message, "err");
    }
    if (id !== run) return;
    control.set(ok ? "ok" : "err");
    options.frame?.set(ok ? "ok" : "err");
    node.dataset.flash = "";
    const holdIcon = ok && options.keepIcon;
    await sleep(holdIcon ? 800 : 500);
    if (id !== run) return;
    delete node.dataset.flash;
    if (!holdIcon) await sleep(300);
    if (id !== run) return;
    control.set("idle");
    options.frame?.set("idle");
    options.after?.(ok);
  };
  node.addEventListener("click", e => { e.preventDefault(); control.run(); });
  return control;
}

/* ---------- OperatorTextField ---------- */
function textField(node) {
  const control = new Control(node);
  const input = node.querySelector("input");
  if (input && input.type === "password") {
    const eye = el("button", {type: "button", class: "eye", "aria-label": "Показати пароль"}, icon("eye"));
    eye.onclick = () => {
      const hidden = input.type === "password";
      input.type = hidden ? "text" : "password";
      eye.replaceChildren(icon(hidden ? "eyeOff" : "eye"));
      input.focus();
    };
    node.append(eye);
  }
  return control;
}
document.querySelectorAll("[data-field]").forEach(textField);

/* ---------- messages and calls ---------- */
function say(node, text, kind) {
  node.className = "msg" + (kind ? " " + kind : "");
  node.replaceChildren();
  if (!text) return;
  if (kind === "wait") node.append(el("span", {class: "badge pending"}));
  node.append(el("span", {}, text));
}

/* A call to this server; a lost login shows the login form. */
async function api(path, fields) {
  const init = fields ? {method: "POST", body: new URLSearchParams(fields)} : {cache: "no-store"};
  let response;
  try { response = await fetch(path, init); } catch (e) { throw new Error("Немає зв'язку із сервером"); }
  const data = await response.json().catch(() => ({error: "Сервер не відповів"}));
  if (response.status === 401 && data.login) { showLogin(); throw new Error("Потрібно увійти знову"); }
  if (!response.ok) throw Object.assign(new Error(data.error || "Помилка " + response.status), data);
  return data;
}

/* ---------- login ---------- */
let captchaId = "", polling = null;
/* Set by "Відкрити панель" while the device is still away: open its panel once it shows up. */
let opening = new URLSearchParams(location.search).get("open");
history.replaceState(null, "", "/admin");

async function newCaptcha() {
  const {id} = await api("/admin/api/captcha");
  captchaId = id;
  $("captcha-img").src = "/admin/captcha/" + id;
  $("captcha").value = "";
}

function showLogin() {
  clearInterval(polling);
  polling = null;
  $("s-devices").classList.add("hidden");
  $("s-login").classList.remove("hidden");
  $("sub").textContent = "Вхід адміністратора";
  newCaptcha().catch(e => say($("login-msg"), e.message, "err"));
  $("password").focus();
}

const loginButton = actionButton($("login-btn"), async () => {
  say($("login-msg"), "");
  try {
    await api("/admin/login", {password: $("password").value, captcha_id: captchaId, captcha: $("captcha").value});
  } catch (e) {
    // Every try uses the captcha up, so a new one comes with the error.
    newCaptcha().catch(() => {});
    $(e.field === "password" ? "password" : "captcha").focus();
    throw e;
  }
}, {msg: () => $("login-msg"), keepIcon: true, after: ok => {
  if (!ok) return;
  $("password").value = "";
  showDevices();
}});
$("s-login").onsubmit = e => { e.preventDefault(); loginButton.run(); };
new Control($("captcha-new"));
$("captcha-new").onclick = () => newCaptcha().catch(e => say($("login-msg"), e.message, "err"));

/* ---------- devices ---------- */
const cards = new Map();

function ago(seconds) {
  if (seconds < 60) return seconds + " с тому";
  if (seconds < 3600) return Math.floor(seconds / 60) + " хв тому";
  if (seconds < 86400) return Math.floor(seconds / 3600) + " год тому";
  return Math.floor(seconds / 86400) + " д тому";
}

function showDevices() {
  $("s-login").classList.add("hidden");
  $("s-devices").classList.remove("hidden");
  $("sub").textContent = "Пристрої · оновлюється кожні " + POLL_MS / 1000 + " с";
  refresh().then(() => { if (opening) cards.get(opening)?.open.run(); });
  clearInterval(polling);
  polling = setInterval(refresh, POLL_MS);
}

async function refresh() {
  let devices;
  try { devices = await api("/admin/api/devices"); } catch (e) { return; }
  const seen = new Set(devices.map(d => d.id));
  for (const [id, card] of cards) if (!seen.has(id)) { card.node.remove(); cards.delete(id); }
  devices.forEach(d => {
    const card = cards.get(d.id) || makeCard(d.id);
    card.device = d;
    renderCard(card);
    $("devices").append(card.node);
  });
  $("no-devices").classList.toggle("hidden", devices.length > 0);
  const ready = opening && devices.find(d => d.id === opening && d.online);
  if (ready) location.href = "/";
}

function makeCard(id) {
  const node = $("device-card").content.firstElementChild.cloneNode(true);
  const pick = name => node.querySelector("[data-" + name + "]");
  const card = {node, pick, msg: pick("msg")};
  card.open = actionButton(pick("open"), async () => {
    say(card.msg, "");
    const {online} = await api(`/admin/devices/${id}/open`, {});
    if (online) { location.href = "/"; return; }
    opening = id;
    card.device.requested = true;
  }, {msg: () => card.msg, keepIcon: true, after: () => renderCard(card)});
  actionButton(pick("end"), async () => {
    say(card.msg, "");
    await api(`/admin/devices/${id}/end`, {});
    if (opening === id) opening = null;
    await refresh();
  }, {msg: () => card.msg});
  new Control(pick("forget")).node.onclick = () => askForget(card);
  waitingSwitch(card, id);
  cards.set(id, card);
  return card;
}

/* OperatorAsyncSwitch: pending until the server answers, then moves and flashes. */
function waitingSwitch(card, id) {
  const node = card.pick("waiting");
  node.onclick = async () => {
    const target = node.getAttribute("aria-checked") !== "true";
    say(card.msg, "");
    node.dataset.state = "pending";
    try {
      await api(`/admin/devices/${id}/waiting`, {on: target ? "1" : ""});
      card.device.waiting = target;
      node.setAttribute("aria-checked", String(target));
      node.dataset.state = "ok";
    } catch (e) {
      node.dataset.state = "err";
      say(card.msg, e.message, "err");
    }
    setTimeout(() => { if (node.dataset.state !== "pending") delete node.dataset.state; }, 800);
  };
}

function renderCard(card) {
  const d = card.device, pick = card.pick;
  card.node.querySelector(".badge").classList.toggle("online", d.online);
  pick("name").textContent = d.name || "без назви";
  pick("meta").textContent = d.id + (d.version ? " · v" + d.version : "");
  pick("seen").textContent = ago(d.seen);
  const session = pick("session");
  session.textContent = d.online ? "відкрита" : d.requested ? "запитано" : "немає";
  session.className = "v" + (d.online ? " ok" : d.requested ? " pending" : "");
  const waiting = pick("waiting");
  if (waiting.dataset.state !== "pending") waiting.setAttribute("aria-checked", String(d.waiting));
  pick("end").classList.toggle("hidden", !d.online);
  if (opening === d.id && !d.online && card.open.state === "idle") {
    say(card.msg, "Сесію запитано. Пристрій відкриє її на найближчій перевірці: за кілька хвилин, якщо ввімкнено "
      + "«Чекаю пристрій», інакше до години. Панель відкриється сама.", "wait");
  }
}

/* ---------- forgetting (OperatorConfirmDialog) ---------- */
const dialog = $("forget-dialog");
const dialogFrame = new Control(dialog);
new Control($("forget-cancel"));
let forgetting = null;
function askForget(card) {
  forgetting = card;
  $("forget-text").textContent = `«${card.device.name || "без назви"}» зникне зі списку. Якщо пристрій ще працює, `
    + "він з'явиться знову на наступній перевірці.";
  say($("forget-msg"), "");
  dialog.showModal();
}
$("forget-cancel").onclick = () => dialog.close();
actionButton($("forget-btn"), async () => {
  $("forget-cancel").disabled = true;
  try {
    await api(`/admin/devices/${forgetting.device.id}/forget`, {});
  } finally { $("forget-cancel").disabled = false; }
}, {msg: () => $("forget-msg"), danger: true, frame: dialogFrame, after: ok => {
  if (!ok) return;
  dialog.close();
  refresh();
}});

/* ---------- start ---------- */
api("/admin/api/devices").then(showDevices, () => {});
