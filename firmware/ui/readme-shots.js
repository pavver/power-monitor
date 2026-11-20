// Takes the README screenshots (docs/screens-light.png, docs/screens-dark.png)
// from the page on the mock device of ui-check.js: three phone screens side by
// side, dashboard, messages and battery. Private values are made up.
//
//   PLAYWRIGHT_CORE=/path/to/playwright-core node firmware/ui/readme-shots.js <out-dir>
//
// Writes <out-dir>/<scheme>-<n>.png; firmware/ui/readme-shots.py joins them.
const path = require("path");
const OUT = process.argv[2];
// ui-check reads the page and the font next to it from its own arguments.
process.argv = [process.argv[0], __filename, path.join(__dirname, "index.html"), OUT];
const { chromium } = require(process.env.PLAYWRIGHT_CORE || "playwright-core");
const { device, route, exe, BMS_READING } = require("./ui-check.js");

const WIDTH = 390;
const HEIGHT = 760;

function home() {
  const dev = device();
  dev.stage = "dashboard"; dev.setupAp = false;
  dev.link = { state: "connected", ssid: "Home", ip: "192.168.1.50" };
  dev.remote = { ...dev.remote, enabled: true, server: "https://svitlo.example.com", phase: "waiting", next_in_s: 3480, last_contact_s: 120 };
  const state = dev.state.bind(dev);
  dev.state = () => ({
    ...state(), power: { present: true, lasted: "3 год 40 хв" },
    telegram: "останнє повідомлення доставлено", chat_id: -1001234567890, thread_id: 7,
  });
  dev.bms = {
    bluetooth: true, scanning: false, found: [], error: null, link: "connected",
    battery: { name: "JK-B2A24S", address: "C8:47:8C:F2:7E:03" },
    device: { model: "JK_B2A24S15P", hardware: "19A", software: "19.13" },
    reading: BMS_READING, bmsPolls: 0,
  };
  dev.bmsPolls = 0;
  return dev;
}

async function shoot(scheme) {
  const browser = await chromium.launch({ executablePath: exe });
  const page = await browser.newPage({ viewport: { width: WIDTH, height: HEIGHT }, deviceScaleFactor: 2, colorScheme: scheme });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  await route(page, home());
  // The mock's preview only roughly follows the templates: the status as the
  // device renders it from the BMS reading above.
  await page.route("**/api/template/preview", r => r.fulfill({
    status: 200, contentType: "application/json",
    body: JSON.stringify({ text: "Дім: Мережа увімкнена ✅\n🔋 85% (4.28 кВт·год)\n🔻 Розряджається: -1.5 А (-80 Вт)\n\n🕒 Оновлено о 14:30" }),
  }));
  await page.goto("http://192.168.1.50/");
  await page.waitForSelector("#s-dashboard.on");
  await page.waitForTimeout(800);
  await page.screenshot({ path: `${OUT}/${scheme}-1.png` });
  await page.$eval("#messages-card", card => window.scrollTo(0, card.getBoundingClientRect().top + window.scrollY - 70));
  await page.waitForTimeout(600);
  await page.screenshot({ path: `${OUT}/${scheme}-2.png` });
  await page.click('[data-go="bms"]');
  await page.waitForSelector("#bms-live:not(.hidden)");
  await page.waitForTimeout(800);
  await page.screenshot({ path: `${OUT}/${scheme}-3.png` });
  await browser.close();
  if (errors.length) throw new Error(errors.join("; "));
}

(async () => {
  for (const scheme of ["light", "dark"]) await shoot(scheme);
})().catch(e => { console.error(e); process.exit(1); });
