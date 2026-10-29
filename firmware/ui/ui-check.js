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
    state() {
      return {
        stage: this.stage, link: this.link, setup_ap: this.setupAp ? { ssid: "PowerMonitor-Setup", portal: "http://192.168.71.1/" } : null, power: { present: true, lasted: "5 хв" },
        telegram: "бот не налаштовано", chat_id: null, pending: 0, password_set: false,
        messages: { device_name: "Дім", start_present: "{device}: запуск, світло є", start_absent: "{device}: запуск",
          lost: "{device}: світло зникло ({duration})", returned: "{device}: світло є ({duration})" },
        firmware: { version: "0.1.0", built: "", slot: "ota_0" },
      };
    },
  };
  return d;
}

async function run(scenario) {
  const browser = await chromium.launch({ executablePath: exe });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2 });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  const dev = device();
  await page.route("**/*", async route => {
    const url = new URL(route.request().url());
    const json = body => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname === "/") return route.fulfill({ status: 200, contentType: "text/html", body: PAGE });
    if (url.pathname === "/operator-mono.woff") return route.fulfill({ status: 200, contentType: "font/woff", body: fs.readFileSync(path.join(path.dirname(process.argv[2]), "operator-mono.woff")) });
    if (url.pathname === "/api/state") return json(dev.state());
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
    return route.fulfill({ status: 404, body: "{}" });
  });
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
})().catch(e => { console.error(e); process.exit(1); });
