// Drives firmware/ui/index.html in headless Chromium against a fake device API.
// Usage: PLAYWRIGHT_CORE=/path/to/playwright-core node ui-check.js index.html out-dir
const { chromium } = require(process.env.PLAYWRIGHT_CORE || "playwright-core");
const fs = require("fs");
const path = require("path");

const PAGE = fs.readFileSync(process.argv[2], "utf8");
const OUT = process.argv[3];
const exe = fs.readdirSync(process.env.HOME + "/.cache/ms-playwright/chromium_headless_shell-1228")
  .filter(d => d.startsWith("chrome")).map(d => path.join(process.env.HOME, ".cache/ms-playwright/chromium_headless_shell-1228", d, "chrome-headless-shell"))[0];

const DEFAULT_TEMPLATES = [
  { name: "svitlo", text: "{mains=on?Мережа увімкнена ✅}{mains=off?Мережа вимкнена ❌}{!mains?⚠️ Датчик мережі недоступний}" },
  { name: "battery_icon", text: "{!bms_soc?❔}{bms_soc>=80?🔋}{bms_soc<80?{bms_soc>=50?🔋½}{bms_soc<50?{bms_soc>=20?🪫}{bms_soc<20?🪫‼️}}}" },
  { name: "battery", text: "{@battery_icon} {bms_soc?{bms_soc}% ({bms_energy} кВт·год)}{!bms_soc?{battery_voltage?{battery_voltage} В}{!battery_voltage?⚠️ Дані батареї недоступні}}" },
  { name: "load", text: "{bms_current>0?⚡ Заряджається: {bms_current} А ({bms_power} Вт)}{bms_current<0?🔻 Розряджається: {bms_current} А ({bms_power} Вт)}{bms_current=0?💤 Без струму}" },
  { name: "update_time", text: "{time?🕒 Оновлено о {time}}" },
  { name: "status", text: "{device}: {@svitlo}\n{@battery}{bms_current?\n{@load}}{time?\n\n{@update_time}}" },
];

const DEFAULT_AUTOMATIONS = [
  {
    on: "change",
    sensor: "ip",
    if: [],
    do: "send",
    text: "🌐 {device}: нова адреса панелі http://{ip}/"
  }
];

const DEFAULT_STATUS_CONFIG = {
  every: 300,
  important: ["mains"],
  bottom: true
};

function device() {
  const d = {
    link: { state: "idle" }, stage: "wifi", setupAp: true, closed: false,
    remote: { id: "0123456789abcdef", enabled: false, server: null, phase: "off", next_in_s: null, last_contact_s: null, error: null },
    messagesSaved: [],
    texts: {
      values: [
        "device", "mains", "ip", "battery_voltage",
        "bms_soc", "bms_voltage", "bms_energy", "bms_current", "bms_power",
        "time", "date"
      ],
      templates: JSON.parse(JSON.stringify(DEFAULT_TEMPLATES)),
      automations: JSON.parse(JSON.stringify(DEFAULT_AUTOMATIONS)),
      status: { ...DEFAULT_STATUS_CONFIG },
      max_templates: 32,
      max_automations: 16
    },
    clock: {
      tz: "EET-2EEST,M3.5.0/3,M10.5.0/4",
      time: "14:30",
      date: "20.11.2026"
    },
    state() {
      return {
        stage: this.stage, link: this.link, setup_ap: this.setupAp ? { ssid: "PowerMonitor-Setup", portal: "http://192.168.71.1/" } : null, power: { present: true, lasted: "5 хв" },
        telegram: "бот не налаштовано", chat_id: null, pending: 0, password_set: false,
        messages: { device_name: "Дім" },
        firmware: { version: "0.1.0", built: "", slot: "ota_0" },
        remote: this.remote,
      };
    },
  };
  return d;
}

function route(page, dev) {
  return page.route("**/*", async route => {
    const url = new URL(route.request().url());
    const json = body => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname === "/") return route.fulfill({ status: 200, contentType: "text/html", body: PAGE });
    if (url.pathname === "/operator-mono.woff") return route.fulfill({ status: 200, contentType: "font/woff", body: fs.readFileSync(path.join(path.dirname(process.argv[2]), "operator-mono.woff")) });
    if (url.pathname === "/api/state") {
      // Answers what the device knew when asked, late: like a slow link.
      const answer = dev.state();
      if (dev.slowState) await new Promise(r => setTimeout(r, dev.slowState));
      return json(answer);
    }
    if (url.pathname === "/api/networks") return json([{ ssid: "Home", rssi: -50, secure: true }, { ssid: "Cafe", rssi: -80, secure: false }]);
    if (url.pathname === "/api/wifi") {
      const form = new URLSearchParams(route.request().postData());
      dev.link = { state: "connecting", ssid: form.get("ssid") };
      setTimeout(() => {
        dev.link = form.get("password") === "right"
          ? { state: "connected", ssid: form.get("ssid"), ip: "192.168.1.50" }
          : { state: "failed", ssid: form.get("ssid"), reason: "не вдалося підключитися — неправильний пароль або мережа відхилила підключення" };
        if (dev.link.state === "connected") dev.stage = "bot";
      }, 3000);
      return json("connecting");
    }
    if (url.pathname === "/api/setup/close") { dev.closed = true; return json("closing"); }
    if (url.pathname === "/api/bot/check") return json({ username: "svitlo_bot" });
    if (url.pathname === "/api/bot/chats") return json([
      { id: -500, thread_id: 7, title: "Будинок › Світло" },
      { id: 42, thread_id: null, title: "Олена" },
    ]);
    if (url.pathname === "/api/bms") {
      if (route.request().method() === "POST") {
        dev.bmsSaved = Object.fromEntries(new URLSearchParams(route.request().postData()));
        dev.bms.battery = { name: dev.bmsSaved.name, address: dev.bmsSaved.address };
        dev.bms.link = "connected";
        dev.bms.reading = BMS_READING;
        return json("saved");
      }
      dev.bmsPolls++;
      return json(dev.bms);
    }
    if (url.pathname === "/api/bms/scan") {
      dev.bms.scanning = true;
      setTimeout(() => {
        dev.bms.scanning = false;
        dev.bms.found = [{ name: "JK-B2A24S", address: "C8:47:8C:F2:7E:03", random: false, rssi: -60, likely: true }];
      }, 1500);
      return json("scanning");
    }
    if (url.pathname === "/api/remote") {
      dev.remoteSaved = Object.fromEntries(new URLSearchParams(route.request().postData()));
      const on = dev.remoteSaved.enabled === "1";
      dev.remote = { ...dev.remote, enabled: on, server: dev.remoteSaved.server, phase: on ? "waiting" : "off", next_in_s: on ? 3540 : null };
      return json("saved");
    }
    if (url.pathname === "/api/remote/connect") { dev.remote = { ...dev.remote, phase: "session", next_in_s: null, last_contact_s: 0 }; return json("connecting"); }
    if (url.pathname === "/api/remote/end") { dev.remote = { ...dev.remote, phase: "waiting", next_in_s: 3600 }; dev.remoteEnded = true; return json("ended"); }
    if (url.pathname === "/api/divider") return json(dev.divider || { divider: null, voltage_mv: null, noisy: false, spread_mv: null });
    if (url.pathname === "/api/messages") {
      dev.messagesSaved.push({ field: url.searchParams.get("field"), value: route.request().postData() });
      if (url.searchParams.get("field") === "device_name") {
        dev.state().messages.device_name = route.request().postData();
      }
      return json("saved");
    }
    if (url.pathname === "/api/texts") return json(dev.texts);
    if (url.pathname === "/api/values") {
      return json(Object.fromEntries(dev.texts.values.map(n => [n, n === "mains" ? "on" : n === "bms_soc" ? "85" : n === "bms_energy" ? "4.28" : null])));
    }
    if (url.pathname === "/api/template") {
      const name = url.searchParams.get("name");
      const text = route.request().postData();
      if (text.includes("{nope}")) return route.fulfill({ status: 400, contentType: "application/json", body: JSON.stringify({ error: "невідома змінна" }) });
      const existing = dev.texts.templates.find(t => t.name === name);
      if (existing) existing.text = text;
      else dev.texts.templates.push({ name, text });
      dev.templateSaved = { name, text };
      return json("saved");
    }
    if (url.pathname === "/api/template/delete") {
      const name = url.searchParams.get("name");
      if (name === "battery_icon" || name === "svitlo") {
        return route.fulfill({ status: 400, contentType: "application/json", body: JSON.stringify({ error: "Шаблон «" + name + "» використовується в шаблоні" }) });
      }
      dev.texts.templates = dev.texts.templates.filter(t => t.name !== name);
      dev.templateDeleted = name;
      return json("deleted");
    }
    if (url.pathname === "/api/template/preview") {
      const template = route.request().postData();
      if (template.includes("{nope}")) return route.fulfill({ status: 400, contentType: "application/json", body: JSON.stringify({ error: "невідома змінна" }) });
      let rendered = template
        .replace(/\{device\}/g, dev.state().messages.device_name)
        .replace(/\{@svitlo\}/g, "Мережа увімкнена ✅")
        .replace(/\{@battery_icon\}/g, "🔋")
        .replace(/\{@battery\}/g, "🔋 85% (4.28 кВт·год)")
        .replace(/\{@load\}/g, "⚡ Заряджається: 1.5 А (80 Вт)")
        .replace(/\{@update_time\}/g, "🕒 Оновлено о 14:30")
        .replace(/\{time\?[^}]*\}/g, "🕒 Оновлено о 14:30")
        .replace(/\{bms_current\?[^}]*\}/g, "\n⚡ Заряджається: 1.5 А (80 Вт)")
        .replace(/\{bms_soc\}/g, "85")
        .replace(/\{ip\}/g, "192.168.1.50")
        .replace(/\{battery_voltage\}/g, "12.45")
        .replace(/\{mains=on\?([^}]*)\}/g, "$1")
        .replace(/\{mains=off\?[^}]*\}/g, "")
        .replace(/\\n/g, "\n");
      return json({ text: rendered });
    }
    if (url.pathname === "/api/automation") {
      const indexStr = url.searchParams.get("index");
      const form = new URLSearchParams(route.request().postData());
      const auto = {
        on: form.get("on"),
        sensor: form.get("sensor") || undefined,
        value: form.get("value") || undefined,
        step: form.get("step") ? Number(form.get("step")) : undefined,
        threshold: form.get("threshold") ? Number(form.get("threshold")) : undefined,
        hysteresis: form.get("hysteresis") ? Number(form.get("hysteresis")) : undefined,
        at: form.get("at") || undefined,
        every: form.get("every") ? Number(form.get("every")) : undefined,
        if: form.getAll("if"),
        do: form.get("do") || "send",
        text: form.get("text") || "",
      };
      if (indexStr !== null && indexStr !== "") {
        dev.texts.automations[parseInt(indexStr, 10)] = auto;
      } else {
        dev.texts.automations.push(auto);
      }
      dev.automationSaved = auto;
      return json("saved");
    }
    if (url.pathname === "/api/automation/delete") {
      const idx = parseInt(url.searchParams.get("index"), 10);
      dev.texts.automations.splice(idx, 1);
      dev.automationDeleted = idx;
      return json("deleted");
    }
    if (url.pathname === "/api/status") {
      const form = new URLSearchParams(route.request().postData());
      dev.texts.status = {
        every: Number(form.get("every")),
        important: (form.get("important") || "").split(",").filter(Boolean),
        bottom: form.get("bottom") === "1" || form.get("bottom") === "true",
      };
      dev.statusSaved = dev.texts.status;
      return json("saved");
    }
    if (url.pathname === "/api/clock") {
      if (route.request().method() === "POST") {
        const form = new URLSearchParams(route.request().postData());
        dev.clock.tz = form.get("tz") || route.request().postData();
        dev.clockSaved = dev.clock.tz;
        return json("saved");
      }
      return json(dev.clock);
    }
    if (url.pathname === "/api/test") {
      dev.testSent = true;
      return json("sent");
    }
    if (url.pathname === "/api/bot") {
      dev.saved = Object.fromEntries(new URLSearchParams(route.request().postData()));
      dev.stage = "dashboard";
      return json("saved");
    }
    return route.fulfill({ status: 404, body: "{}" });
  });
}

const BMS_READING = {
  age_s: 1, cells_mv: [3327, 3329, 3331, 3330],
  values: { bms_soc: "85", bms_voltage: "53.22", bms_energy: "4.28", bms_current: "-1.5", bms_power: "-80", bms_remaining: "42.8",
    bms_capacity: "50.0", bms_cycles: "3", bms_cell_min: "3.327", bms_cell_max: "3.331", bms_cell_delta: "0.004",
    bms_temp1: "20.7", bms_temp2: "20.7", bms_mosfet: "23.4", bms_charging: "так", bms_discharging: "ні" },
};

/* Battery screen: the BMS is asked only while its screen is open. */
/* Picks an option the way a user does: the DropdownMenu field, then the row. */
async function pick(page, select, value) {
  const index = await page.$eval(select, (node, v) => [...node.options].findIndex(o => o.value === v), value);
  await page.click(select + " + .op-dropdown-anchor .op-dropdown-trigger");
  await page.click(`.op-dropdown-menu .op-dropdown-item >> nth=${index}`);
  const now = await page.$eval(select, node => node.value);
  if (now !== value) throw new Error(`${select}: picked ${value}, has ${now}`);
}

async function runBms() {
  const browser = await chromium.launch({ executablePath: exe });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2 });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  const dev = device();
  dev.stage = "dashboard"; dev.setupAp = false; dev.link = { state: "connected", ssid: "Home", ip: "192.168.1.50" };
  dev.bms = { battery: null, bluetooth: true, link: "off", error: null, device: null, reading: null, scanning: false, found: [] };
  dev.bmsPolls = 0;
  // A divider set but nothing fitted: the pin is noise.
  dev.divider = { divider: { top_ohm: 330000, bottom_ohm: 10000 }, voltage_mv: null, noisy: true, spread_mv: 900 };
  await route(page, dev);
  // Plain http: the page copies through a hidden textarea; record what it copied.
  await page.addInitScript(() => {
    const copy = document.execCommand.bind(document);
    document.execCommand = (cmd, ...rest) => {
      if (cmd !== "copy") return copy(cmd, ...rest);
      window.copied = document.activeElement.value;
      return copy(cmd) || true;
    };
  });
  await page.goto("http://192.168.1.50/");
  await page.waitForFunction(() => document.querySelector("section.on").id === "s-dashboard");
  await page.waitForTimeout(3500);
  const pollsOnDashboard = dev.bmsPolls;
  await page.click("[data-go=bms]");
  await page.waitForSelector("#s-bms.on");
  await page.click("#bms-scan-btn");
  // The button spins for the whole search, not just for the request.
  await page.waitForTimeout(1000);
  const scanButton = await page.$eval("#bms-scan-btn", n => n.dataset.state);
  await page.waitForSelector("#bms-found .op-list-row button", { timeout: 6000 });
  await page.screenshot({ path: OUT + "/bms-0-found.png" });
  await page.click("#bms-found .op-list-row button");
  await page.waitForFunction(() => document.getElementById("bms-head").textContent.includes("85%"), null, { timeout: 6000 });
  const shown = await page.$eval("#bms-values", n => n.textContent);
  await page.click("#bms-values .op-list-row >> nth=0");
  const copied = await page.evaluate(() => window.copied);
  const copiedNote = await page.$eval("#bms-values .op-list-row", n => n.textContent);
  const noise = await page.evaluate(() => [document.getElementById("divider-value").textContent.includes("шум"),
    !document.getElementById("divider-noise").classList.contains("hidden")]);
  await page.screenshot({ path: OUT + "/bms-1-live.png", fullPage: true });
  await page.click("#back");
  await page.waitForSelector("#s-dashboard.on");
  const pollsLeaving = dev.bmsPolls;
  await page.waitForTimeout(4000);
  const pollsAfter = dev.bmsPolls;
  await browser.close();
  const ok = errors.length === 0 && pollsOnDashboard === 0 && pollsAfter === pollsLeaving && pollsLeaving > 0
    && dev.bmsSaved && dev.bmsSaved.address === "C8:47:8C:F2:7E:03" && dev.bmsSaved.random === ""
    && shown.includes("53.22 В")
    && copied === "{bms_soc}" && copiedNote.includes("скопійовано") && scanButton === "pending" && noise[0] && noise[1];
  if (!ok) process.exitCode = 1;
  console.log((ok ? "PASS " : "FAIL ") + "bms", JSON.stringify({ noise, scanButton, pollsOnDashboard, pollsLeaving, pollsAfter, saved: dev.bmsSaved, copied, copiedNote, errors }));
}

/* Messages system: status preview, template editing & preview, bad template error, automation save, and theme screenshots. */
async function runMessages() {
  const browser = await chromium.launch({ executablePath: exe });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2 });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  const dev = device();
  dev.stage = "dashboard"; dev.setupAp = false; dev.link = { state: "connected", ssid: "Home", ip: "192.168.1.50" };
  await route(page, dev);
  await page.goto("http://192.168.1.50/");
  await page.waitForSelector("#s-dashboard.on");

  // 1. Status preview renders and status config can be changed and tested
  await page.waitForFunction(() => document.getElementById("status-preview").textContent.length > 0, null, { timeout: 5000 });
  const statusPreview = await page.$eval("#status-preview", n => n.textContent);
  const statusEvery = await page.inputValue("#status-every");
  // The status template opens from the status section.
  await page.click("#status-edit");
  await page.waitForSelector("#s-text.on");
  const statusTemplate = await page.inputValue("#t-name");
  await page.click("#back");
  await page.waitForSelector("#s-dashboard.on");
  // TextField: SAVE only while the value differs, and no empty trailing gap.
  const saveHidden = !await page.isVisible("#device-save");
  await page.fill("#m-device_name", "Дім 2");
  const saveShown = await page.isVisible("#device-save");
  const saveFlush = await page.$eval("#device-field", f => Math.abs(f.getBoundingClientRect().right - f.querySelector(".op-field-save-slot").getBoundingClientRect().right) < 1);
  await page.fill("#m-device_name", "Дім");
  const saveGone = !await page.isVisible("#device-save");
  const fieldSave = {saveHidden, saveShown, saveFlush, saveGone};
  await page.fill("#status-every", "120");
  const box = () => page.$eval("#status-bottom", n => n.getAttribute("aria-checked") + "/" + n.querySelector(".op-checkbox-box").dataset.state);
  const bottomBefore = await box();
  await page.click("#status-bottom"); // toggles to false
  await page.waitForFunction(() => document.querySelector("#status-bottom .op-checkbox-box").dataset.state === "success", null, { timeout: 5000 });
  const bottomAfter = await box();
  await page.waitForFunction(() => document.querySelector("#status-bottom .op-checkbox-box").dataset.state === "idle", null, { timeout: 5000 });
  const checkbox = {bottomBefore, bottomAfter, bottomRest: await box()};
  await page.click("#status-save");
  await page.waitForFunction(() => document.getElementById("status-save").dataset.state === "success", null, { timeout: 3000 });

  // Test send button
  await page.click("#status-test");
  await page.waitForFunction(() => document.getElementById("status-test").dataset.state === "success", null, { timeout: 3000 });

  // 2. Templates list renders standard templates
  await page.waitForSelector("#templates-list .op-list-row");
  const templateRows = await page.$$eval("#templates-list .op-list-row", n => n.map(r => r.querySelector(".op-list-title").textContent));

  // 3. Open template editor for "svitlo"
  await page.click('#templates-list .op-list-row:has-text("svitlo")');
  await page.waitForSelector("#s-text.on");
  const nameReadonly = await page.$eval("#t-name", n => n.readOnly);

  // Highlight check with {@call} and {cond?…}
  await page.fill("#t-area", "{@load} {battery_voltage<12?низька} {battery_voltage}");
  await page.waitForTimeout(400);
  const litVars = await page.$$eval("#t-hl .v", n => n.map(s => s.textContent));
  const litCond = await page.$$eval("#t-hl .c", n => n.map(s => s.textContent));

  // 4. Bad template shows device's error in preview and highlight
  await page.fill("#t-area", "{nope}");
  await page.waitForFunction(() => document.getElementById("t-preview").classList.contains("bad") && document.getElementById("t-preview").textContent.includes("невідома"), null, { timeout: 4000 });
  const badPreviewText = await page.$eval("#t-preview", p => p.textContent);
  const flagged = await page.$$eval("#t-hl .x", n => n.map(s => s.textContent));

  // Saving bad template shows error
  await page.click("#t-save");
  await page.waitForFunction(() => document.getElementById("t-save").dataset.state === "error", null, { timeout: 3000 });
  const badSaveMsg = await page.$eval("#t-msg", m => m.textContent);

  // 5. Template edit saves
  const validEdit = "Мережа є! {@update_time}";
  await page.fill("#t-area", validEdit);
  await page.click("#t-save");
  await page.waitForFunction(() => document.getElementById("t-save").dataset.state === "success", null, { timeout: 3000 });

  // 6. Delete template that is in use shows device error
  await page.click("#t-delete");
  await page.waitForFunction(() => document.getElementById("t-delete").dataset.state === "error", null, { timeout: 3000 });
  const deleteInUseMsg = await page.$eval("#t-msg", m => m.textContent);

  // Back to dashboard
  await page.click("#back");
  await page.waitForSelector("#s-dashboard.on");

  // 7. Automations list renders and new automation saves
  const autoRows = await page.$$eval("#automations-list .op-list-row", n => n.map(r => r.querySelector(".op-list-title").textContent));
  // The important-sensor picker: current values in brackets after the
  // names, and the list scrolls under the finger instead of closing.
  await page.click("#imp-pick + .op-dropdown-anchor .op-dropdown-trigger");
  await page.waitForFunction(() => [...document.querySelectorAll(".op-dropdown-item")].some(i => i.textContent.endsWith(")")));
  const details = await page.$$eval(".op-dropdown-item", items => items.map(i => i.textContent));
  if (!details.includes("bms_soc (85 %)") || !details.includes("bms_energy (4.28 кВт·год)")) throw new Error("no current value in the picker: " + details);
  const scroller = ".op-dropdown-menu .op-scroll-contain";
  const listBox = await page.$eval(scroller, n => { const r = n.getBoundingClientRect(); return {x: r.x + r.width / 2, y: r.y + r.height / 2, scrollable: n.scrollHeight > n.clientHeight}; });
  await page.mouse.move(listBox.x, listBox.y);
  await page.mouse.wheel(0, 200);
  await page.waitForTimeout(300);
  const stillOpen = await page.$(scroller) !== null;
  const scrolled = await page.$eval(scroller, n => n.scrollTop).catch(() => -1);
  if (!stillOpen || (listBox.scrollable && scrolled <= 0)) throw new Error(`picker closed or did not scroll (open ${stillOpen}, top ${scrolled})`);
  await page.screenshot({ path: OUT + "/messages-4-picker.png" });
  await page.keyboard.press("Escape");
  await page.waitForSelector(".op-dropdown-menu", { state: "detached" });

  await page.click("#auto-add");
  await page.waitForSelector("#s-automation.on");
  await pick(page, "#a-on", "below");
  await pick(page, "#a-sensor", "battery_voltage");
  await page.fill("#a-thresh", "11.5");
  await page.fill("#a-hyst", "0.2");
  await page.click("#a-cond-add");
  await page.waitForSelector("#a-conditions input");
  await page.fill("#a-conditions input", "mains=off");
  await pick(page, "#a-do", "send");
  await page.fill("#a-area", "Акумулятор розряджено: {battery_voltage} В");
  await page.click("#a-save");
  await page.waitForSelector("#s-dashboard.on");
  await page.waitForTimeout(500);
  const autoRowsAfter = await page.$$eval("#automations-list .op-list-row", n => n.map(r => r.querySelector(".op-list-title").textContent));

  // 8. Screenshots in both themes
  // Light theme
  await page.emulateMedia({ colorScheme: "light" });
  await page.$eval("#messages-card", n => n.scrollIntoView());
  await page.screenshot({ path: OUT + "/messages-0-light.png" });

  // Dark theme
  await page.emulateMedia({ colorScheme: "dark" });
  await page.waitForTimeout(600); // colour transitions settle before the shot
  await page.screenshot({ path: OUT + "/messages-1-dark.png" });

  // Open template in dark theme
  await page.click('#templates-list .op-list-row:has-text("svitlo")');
  await page.waitForSelector("#s-text.on");
  await page.screenshot({ path: OUT + "/messages-2-template-dark.png", fullPage: true });

  // Open automation in dark theme
  await page.click("#back");
  await page.waitForSelector("#s-dashboard.on");
  await page.click("#automations-list .op-list-row >> nth=0");
  await page.waitForSelector("#s-automation.on");
  await page.screenshot({ path: OUT + "/messages-3-automation-dark.png", fullPage: true });

  await browser.close();

  const ok = errors.length === 0
    && statusPreview.includes("Дім")
    && Object.values(fieldSave).every(Boolean) && statusTemplate === "status"
    && checkbox.bottomBefore === "true/selected" && checkbox.bottomAfter === "false/success" && checkbox.bottomRest === "false/idle"
    && statusEvery === "300"
    && dev.statusSaved && dev.statusSaved.every === 120 && dev.statusSaved.bottom === false
    && dev.testSent === true
    && templateRows.length === 6 && templateRows.includes("svitlo")
    && nameReadonly === true
    && litVars.includes("{@load}") && litCond.some(c => c.includes("battery_voltage<12?"))
    && badPreviewText.includes("невідома") && flagged.includes("{nope}")
    && badSaveMsg.includes("невідома")
    && dev.texts.templates.find(t => t.name === "svitlo")?.text === validEdit
    && deleteInUseMsg.includes("використовується")
    && autoRows.length === 1
    && dev.automationSaved && dev.automationSaved.on === "below" && dev.automationSaved.sensor === "battery_voltage"
      && dev.automationSaved.threshold === 11.5 && dev.automationSaved.hysteresis === 0.2
      && dev.automationSaved.if[0] === "mains=off" && dev.automationSaved.text === "Акумулятор розряджено: {battery_voltage} В"
    && autoRowsAfter.length === 2;

  if (!ok) process.exitCode = 1;
  console.log((ok ? "PASS " : "FAIL ") + "messages", JSON.stringify({
    statusPreview, statusTemplate, fieldSave, checkbox, devStatus: dev.statusSaved, testSent: dev.testSent,
    templateRows: templateRows.length, nameReadonly, litVars, litCond,
    badPreviewText, flagged, badSaveMsg, deleteInUseMsg,
    autoRows: autoRows.length, autoRowsAfter: autoRowsAfter.length,
    automationSaved: dev.automationSaved, errors
  }));
}

/* Remote support: switch on with a server, connect now, see the session, end it. */
async function runRemote() {
  const browser = await chromium.launch({ executablePath: exe });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2 });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  const dev = device();
  dev.stage = "dashboard"; dev.setupAp = false; dev.link = { state: "connected", ssid: "Home", ip: "192.168.1.50" };
  await route(page, dev);
  await page.goto("http://192.168.1.50/");
  await page.waitForSelector("#s-dashboard.on");
  const offTitle = await page.$eval("#remote-title", n => n.textContent);
  dev.slowState = 1500;
  await page.fill("#remote-server", "https://svitlo.example.com");
  await page.evaluate(() => {
    window.ariaLog = [];
    const node = document.getElementById("remote-switch");
    new MutationObserver(() => window.ariaLog.push(node.getAttribute("aria-checked"))).observe(node, { attributes: true, attributeFilter: ["aria-checked"] });
  });
  await page.click("#remote-switch");
  await page.waitForFunction(() => document.getElementById("remote-switch").getAttribute("aria-checked") === "true", null, { timeout: 6000 });
  await page.waitForFunction(() => document.getElementById("remote-sub").textContent.includes("59 хв"), null, { timeout: 6000 });
  await page.waitForTimeout(3500);
  // A state poll that left before the save must not flip the switch back.
  const flips = await page.evaluate(() => window.ariaLog.slice(window.ariaLog.indexOf("true")).filter(v => v !== "true").length);
  await page.$eval("#remote-card", n => n.scrollIntoView());
  await page.screenshot({ path: OUT + "/remote-0-on.png" });
  await page.click("#remote-connect");
  await page.waitForFunction(() => document.getElementById("remote-title").textContent.includes("ПІДКЛЮЧЕНО"), null, { timeout: 6000 });
  const dashboardRow = await page.$eval("#d-remote", n => n.textContent);
  await page.$eval("#remote-card", n => n.scrollIntoView());
  await page.screenshot({ path: OUT + "/remote-1-session.png" });
  await page.click("#remote-end");
  await page.waitForFunction(() => !document.getElementById("remote-title").textContent.includes("ПІДКЛЮЧЕНО"), null, { timeout: 6000 });
  await browser.close();
  const ok = errors.length === 0 && offTitle.includes("ВИМКНЕНО") && dev.remoteSaved
    && dev.remoteSaved.enabled === "1" && dev.remoteSaved.server === "https://svitlo.example.com"
    && dashboardRow.includes("підключено") && dev.remoteEnded && flips === 0;
  if (!ok) process.exitCode = 1;
  console.log((ok ? "PASS " : "FAIL ") + "remote", JSON.stringify({ offTitle, flips, saved: dev.remoteSaved, dashboardRow, ended: dev.remoteEnded, errors }));
}

/* Bot step: choosing a forum topic saves it as the destination. */
async function runBot() {
  const browser = await chromium.launch({ executablePath: exe });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2 });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  const dev = device();
  dev.stage = "bot"; dev.setupAp = false;
  dev.link = { state: "connected", ssid: "Home", ip: "192.168.1.50" };
  await route(page, dev);
  await page.goto("http://192.168.1.50/");
  await page.fill("#token", "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw");
  await page.click("#token-btn");
  await page.waitForSelector("#chats .op-list-row");
  const kinds = await page.$$eval("#chats .op-list-row .op-list-subtitle", nodes => nodes.map(n => n.textContent));
  await page.screenshot({ path: OUT + "/topic-0-chats.png" });
  await page.click("#chats .op-list-row >> nth=0 >> button");
  await page.waitForFunction(() => document.querySelector("section.on").id === "s-dashboard", null, { timeout: 5000 });
  await browser.close();
  const ok = errors.length === 0 && dev.saved && dev.saved.chat_id === "-500" && dev.saved.thread_id === "7"
    && kinds[0].startsWith("гілка форуму");
  if (!ok) process.exitCode = 1;
  console.log((ok ? "PASS " : "FAIL ") + "topic", JSON.stringify({ saved: dev.saved, kinds, errors }));
}

async function run(scenario) {
  const browser = await chromium.launch({ executablePath: exe });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2 });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  const dev = device();
  await route(page, dev);
  // Through the captive portal the page lives on the setup address.
  await page.goto(scenario.portal ? "http://192.168.71.1/" : "http://192.168.1.50/");
  await page.waitForSelector("#networks .op-list-row");
  await page.click("#networks .op-list-row >> nth=0");
  await page.fill("#join-pass", scenario.password);
  await page.screenshot({ path: OUT + "/" + scenario.name + "-0-picked.png" });
  await page.click("#join-btn");
  const states = [];
  const sample = async label => states.push(label + ": " + await page.$eval("#join-btn", b => b.dataset.state + (b.querySelector(".op-btn-content .op-c-success, .op-btn-content .op-c-error") ? "+icon" : "")));
  await page.waitForTimeout(300); await sample("0.3s");
  await page.screenshot({ path: OUT + "/" + scenario.name + "-1-pending.png", clip: { x: 16, y: 280, width: 360, height: 140 } });
  await page.waitForTimeout(2000); await sample("2.3s");
  // The device answers at 3 s; the page polls every second.
  await page.waitForFunction(() => ["success", "error"].includes(document.getElementById("join-btn").dataset.state), null, { timeout: 5000 });
  await sample("result");
  await page.screenshot({ path: OUT + "/" + scenario.name + "-2-result.png" });
  // The result holds 800 ms (flash 500 + tail 300); the screenshot above takes some of it.
  await page.waitForTimeout(400); states.push("+0.4s: " + await page.$eval("#join-btn", b => b.dataset.state)); // icon or label: either is right here
  await page.waitForTimeout(1400); await sample("+1.8s");
  const view = await page.$eval("section.on", s => s.id);
  const font = await page.evaluate(() => document.fonts.check('14px "Operator Mono"') && [...document.fonts].some(f => f.family.includes("Operator Mono") && f.status === "loaded"));
  const msg = await page.$eval("#join-msg", m => m.textContent);
  await page.screenshot({ path: OUT + "/" + scenario.name + "-3-after.png" });
  if (scenario.portal && view === "s-move") {
    await page.click("#close-btn");
    await page.waitForFunction(() => document.getElementById("close-btn").dataset.state === "success", null, { timeout: 3000 });
    await page.screenshot({ path: OUT + "/" + scenario.name + "-4-closing.png" });
  }
  await browser.close();
  const expected = scenario.expect;
  const ok = errors.length === 0 && font && view === expected.view && states.join() === expected.states.join()
    && dev.closed === Boolean(expected.closed);
  if (!ok) process.exitCode = 1;
  console.log((ok ? "PASS " : "FAIL ") + scenario.name, JSON.stringify({ states, view, font, closed: dev.closed, msg, errors }));
}

// The button waits for the device's verdict, not for the request: it stays
// pending while connecting, then flashes the icon, the label, and fades out.
const timeline = result => ["0.3s: pending", "2.3s: pending", "result: " + result + "+icon", "+0.4s: " + result, "+1.8s: idle"];

(async () => {
  await run({ name: "wrong", portal: true, password: "wrong", expect: { view: "s-wifi", states: timeline("error") } });
  // In the portal, success leads to the screen that sends the user home,
  // whose button closes the setup network.
  await run({ name: "portal", portal: true, password: "right", expect: { view: "s-move", states: timeline("success"), closed: true } });
  // From the home network (changing Wi-Fi later) it goes straight on.
  await run({ name: "home", password: "right", expect: { view: "s-bot", states: timeline("success") } });
  await runBot();
  await runBms();
  await runMessages();
  await runRemote();
})().catch(e => { console.error(e); process.exit(1); });
