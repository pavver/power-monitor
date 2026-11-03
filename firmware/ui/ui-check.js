// Drives firmware/ui/index.html in headless Chromium against a fake device API.
// Usage: PLAYWRIGHT_CORE=/path/to/playwright-core node ui-check.js index.html out-dir
const { chromium } = require(process.env.PLAYWRIGHT_CORE || "playwright-core");
const fs = require("fs");
const path = require("path");

const PAGE = fs.readFileSync(process.argv[2], "utf8");
const OUT = process.argv[3];
const exe = fs.readdirSync(process.env.HOME + "/.cache/ms-playwright/chromium_headless_shell-1228")
  .filter(d => d.startsWith("chrome")).map(d => path.join(process.env.HOME, ".cache/ms-playwright/chromium_headless_shell-1228", d, "chrome-headless-shell"))[0];

function device() {
  const d = {
    link: { state: "idle" }, stage: "wifi", setupAp: true, closed: false,
    remote: { enabled: false, server: null, phase: "off", next_in_s: null, last_contact_s: null, error: null },
    state() {
      return {
        stage: this.stage, link: this.link, setup_ap: this.setupAp ? { ssid: "PowerMonitor-Setup", portal: "http://192.168.71.1/" } : null, power: { present: true, lasted: "5 хв" },
        telegram: "бот не налаштовано", chat_id: null, pending: 0, password_set: false,
        messages: { device_name: "Дім", start_present: "{device}: запуск, світло є", start_absent: "{device}: запуск",
          lost: "{device}: світло зникло ({duration})", returned: "{device}: світло є ({duration})" },
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
    if (url.pathname === "/api/divider") return json({ divider: null, voltage_mv: null });
    if (url.pathname === "/api/messages/preview") {
      // The device fills the text; the page only has to show its answer.
      const form = new URLSearchParams(route.request().postData());
      const template = form.get("template");
      if (template.includes("{nope}")) return route.fulfill({ status: 400, contentType: "application/json", body: JSON.stringify({ error: "невідома змінна" }) });
      return json(form.get("device_name") + "|" + form.get("text") + "|" + template);
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
  values: { bms_soc: "85", bms_voltage: "53.22", bms_current: "-1.5", bms_power: "-80", bms_remaining: "42.8",
    bms_capacity: "50.0", bms_cycles: "3", bms_cell_min: "3.327", bms_cell_max: "3.331", bms_cell_delta: "0.004",
    bms_temp1: "20.7", bms_temp2: "20.7", bms_mosfet: "23.4", bms_charging: "так", bms_discharging: "ні" },
};

/* Battery screen: the BMS is asked only while its screen is open. */
async function runBms() {
  const browser = await chromium.launch({ executablePath: exe });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2 });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  const dev = device();
  dev.stage = "dashboard"; dev.setupAp = false; dev.link = { state: "connected", ssid: "Home", ip: "192.168.1.50" };
  dev.bms = { battery: null, bluetooth: true, link: "off", error: null, device: null, reading: null, scanning: false, found: [] };
  dev.bmsPolls = 0;
  await route(page, dev);
  // Plain http: the page copies through a hidden textarea; record what it copied.
  await page.addInitScript(() => {
    const copy = document.execCommand.bind(document);
    document.execCommand = cmd => { if (cmd === "copy") window.copied = document.activeElement.value; return copy(cmd) || true; };
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
  await page.waitForSelector("#bms-found .item button", { timeout: 6000 });
  await page.screenshot({ path: OUT + "/bms-0-found.png" });
  await page.click("#bms-found .item button");
  await page.waitForFunction(() => document.getElementById("bms-head").textContent.includes("85%"), null, { timeout: 6000 });
  const shown = await page.$eval("#bms-values", n => n.textContent);
  await page.click("#bms-values .item >> nth=0");
  const copied = await page.evaluate(() => window.copied);
  const copiedNote = await page.$eval("#bms-values .item", n => n.textContent);
  await page.screenshot({ path: OUT + "/bms-1-live.png", fullPage: true });
  await page.click("#back");
  await page.waitForSelector("#s-dashboard.on");
  const pollsLeaving = dev.bmsPolls;
  await page.waitForTimeout(4000);
  const pollsAfter = dev.bmsPolls;
  const shownPreview = async () => {
    await page.waitForTimeout(800);
    return page.$eval("#m-lost", a => { const p = a.closest("div").parentElement.querySelector(".preview"); return [p.textContent, p.classList.contains("bad")]; });
  };
  await page.fill("#m-lost", "Батарея {bms_soc}%");
  const preview = await shownPreview();
  await page.fill("#m-lost", "{nope}");
  const rejected = await shownPreview();
  const chips = await page.$$eval(".chip", n => n.length);
  await browser.close();
  const ok = errors.length === 0 && pollsOnDashboard === 0 && pollsAfter === pollsLeaving && pollsLeaving > 0
    && dev.bmsSaved && dev.bmsSaved.address === "C8:47:8C:F2:7E:03" && dev.bmsSaved.random === ""
    && shown.includes("53.22 В") && preview[0] === "Дім|lost|Батарея {bms_soc}%" && !preview[1] && rejected[0] === "невідома змінна" && rejected[1]
    && copied === "{bms_soc}" && copiedNote.includes("скопійовано") && chips === 0 && scanButton === "pending";
  if (!ok) process.exitCode = 1;
  console.log((ok ? "PASS " : "FAIL ") + "bms", JSON.stringify({ scanButton, pollsOnDashboard, pollsLeaving, pollsAfter, saved: dev.bmsSaved, preview, rejected, copied, copiedNote, chips, errors }));
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
  await page.waitForSelector("#chats .item");
  const kinds = await page.$$eval("#chats .item .s", nodes => nodes.map(n => n.textContent));
  await page.screenshot({ path: OUT + "/topic-0-chats.png" });
  await page.click("#chats .item >> nth=0 >> button");
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
  await page.waitForSelector("#networks button");
  await page.click("#networks button >> nth=0");
  await page.fill("#join-pass", scenario.password);
  await page.screenshot({ path: OUT + "/" + scenario.name + "-0-picked.png" });
  await page.click("#join-btn");
  const states = [];
  const sample = async label => states.push(label + ": " + await page.$eval("#join-btn", b => b.dataset.state + (b.dataset.flash !== undefined ? "+icon" : "")));
  await page.waitForTimeout(300); await sample("0.3s");
  await page.screenshot({ path: OUT + "/" + scenario.name + "-1-pending.png", clip: { x: 16, y: 280, width: 360, height: 140 } });
  await page.waitForTimeout(2000); await sample("2.3s");
  // The device answers at 3 s; the page polls every second.
  await page.waitForFunction(() => ["ok", "err"].includes(document.getElementById("join-btn").dataset.state), null, { timeout: 5000 });
  await sample("result");
  await page.screenshot({ path: OUT + "/" + scenario.name + "-2-result.png" });
  await page.waitForTimeout(600); await sample("+0.6s");
  await page.waitForTimeout(1200); await sample("+1.8s");
  const view = await page.$eval("section.on", s => s.id);
  const font = await page.evaluate(() => document.fonts.check('14px "Operator Mono"') && [...document.fonts].some(f => f.family.includes("Operator Mono") && f.status === "loaded"));
  const msg = await page.$eval("#join-msg", m => m.textContent);
  await page.screenshot({ path: OUT + "/" + scenario.name + "-3-after.png" });
  if (scenario.portal && view === "s-move") {
    await page.click("#close-btn");
    await page.waitForFunction(() => document.getElementById("close-btn").dataset.state === "ok", null, { timeout: 3000 });
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
const timeline = result => ["0.3s: pending", "2.3s: pending", "result: " + result + "+icon", "+0.6s: " + result, "+1.8s: idle"];

(async () => {
  await run({ name: "wrong", portal: true, password: "wrong", expect: { view: "s-wifi", states: timeline("err") } });
  // In the portal, success leads to the screen that sends the user home,
  // whose button closes the setup network.
  await run({ name: "portal", portal: true, password: "right", expect: { view: "s-move", states: timeline("ok"), closed: true } });
  // From the home network (changing Wi-Fi later) it goes straight on.
  await run({ name: "home", password: "right", expect: { view: "s-bot", states: timeline("ok") } });
  await runBot();
  await runBms();
  await runRemote();
})().catch(e => { console.error(e); process.exit(1); });
