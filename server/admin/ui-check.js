// Drives server/admin in headless Chromium against a fake JSON API.
// Usage: node server/admin/ui-check.js <out-dir>
const { chromium } = require(process.env.PLAYWRIGHT_CORE || "/tmp/claude-1000/-home-pavver-rust-esp/5bfd559e-822e-4e5e-941a-e07d256d5099/scratchpad/pw/node_modules/playwright-core");
const fs = require("fs");
const path = require("path");

const DIST = path.resolve(__dirname, "dist");
const OUT = process.argv[2] || "/tmp/admin-screenshots";

const exe = fs.readdirSync(path.join(process.env.HOME, ".cache/ms-playwright/chromium_headless_shell-1228"))
  .filter(d => d.startsWith("chrome"))
  .map(d => path.join(process.env.HOME, ".cache/ms-playwright/chromium_headless_shell-1228", d, "chrome-headless-shell"))[0];

function createMockServer() {
  return {
    isLoggedIn: false,
    captchaCounter: 0,
    failWaitingOnce: false,
    lastPostedView: null,
    viewPostCount: 0,
    devices: [
      {
        id: "0123456789abcdef",
        name: "Дім",
        version: "0.1.0",
        seen: 15,
        online: true,
        requested: false,
        waiting: false,
        status: "session",
        view: null,
      },
      {
        id: "fedcba9876543210",
        name: "Офіс",
        version: "0.2.0",
        seen: 320,
        online: false,
        requested: false,
        waiting: false,
        status: "alive",
        view: null,
      },
      {
        id: "1122334455667788",
        name: "Гараж",
        version: "0.1.5",
        seen: 7200,
        online: false,
        requested: false,
        waiting: false,
        status: "offline",
        view: null,
      },
    ],
  };
}

async function setupRoutes(page, state) {
  await page.route("**/*", async route => {
    const url = new URL(route.request().url());
    const method = route.request().method();
    const pathname = url.pathname;

    const fulfill = (status, contentType, body) =>
      route.fulfill({ status, contentType, body });

    const json = (status, data) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(data),
      });

    // Mock device panel page at /
    if (pathname === "/" || pathname === "/index.html") {
      const html = `<!DOCTYPE html>
<html lang="uk">
<head>
  <meta charset="utf-8">
  <title>Device Panel</title>
  <style>
    body {
      font-family: monospace;
      padding: 24px;
      background: #222;
      color: #eee;
    }
    .screen-indicator {
      font-size: 20px;
      font-weight: bold;
      color: #8CBF6E;
      margin-bottom: 16px;
    }
    button {
      padding: 8px 16px;
      margin-right: 8px;
      cursor: pointer;
    }
  </style>
</head>
<body>
  <h2>Панель керування пристроєм</h2>
  <div class="screen-indicator">Екран: <span id="current-view">login</span></div>
  <div>
    <button id="btn-login" onclick="show('login')">Вхід</button>
    <button id="btn-wifi" onclick="show('wifi')">Wi-Fi</button>
    <button id="btn-dashboard" onclick="show('dashboard')">Панель</button>
    <button id="btn-bms" onclick="show('bms')">BMS</button>
  </div>
  <script>
    window.recordedViews = ["login"];
    function show(name) {
      window.recordedViews.push(name);
      const el = document.getElementById("current-view");
      if (el) el.textContent = name;
    }
  </script>
</body>
</html>`;
      return fulfill(200, "text/html; charset=utf-8", html);
    }

    // Static assets from DIST
    if (pathname === "/admin" || pathname === "/admin/" || pathname === "/admin/index.html") {
      return fulfill(200, "text/html; charset=utf-8", fs.readFileSync(path.join(DIST, "index.html")));
    }
    if (pathname === "/admin/admin.js") {
      return fulfill(200, "application/javascript", fs.readFileSync(path.join(DIST, "admin.js")));
    }
    if (pathname === "/admin/admin_bg.wasm" || pathname === "/admin/admin.wasm") {
      return fulfill(200, "application/wasm", fs.readFileSync(path.join(DIST, "admin_bg.wasm")));
    }
    if (pathname === "/admin/admin.css") {
      return fulfill(200, "text/css; charset=utf-8", fs.readFileSync(path.join(DIST, "admin.css")));
    }
    if (pathname === "/admin/assets/fonts/CascadiaCode.ttf") {
      return fulfill(200, "font/ttf", fs.readFileSync(path.join(DIST, "assets/fonts/CascadiaCode.ttf")));
    }
    if (pathname === "/admin/assets/fonts/CascadiaCode-OFL.txt") {
      return fulfill(200, "text/plain; charset=utf-8", fs.readFileSync(path.join(DIST, "assets/fonts/CascadiaCode-OFL.txt")));
    }

    // JSON API routes
    if (pathname === "/admin/api/captcha" && method === "GET") {
      state.captchaCounter++;
      return json(200, { id: `cap-${state.captchaCounter}` });
    }

    if (pathname.startsWith("/admin/captcha/") && method === "GET") {
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="64"><rect width="100%" height="100%" fill="#eee"/><text x="40" y="42" font-size="28" font-family="monospace">12345</text></svg>`;
      return fulfill(200, "image/svg+xml", svg);
    }

    if (pathname === "/admin/login" && method === "POST") {
      const postData = route.request().postData() || "";
      const params = new URLSearchParams(postData);
      const password = params.get("password");
      const captcha = params.get("captcha");

      if (captcha !== "12345") {
        return json(403, {
          field: "captcha",
          error: "Циفري з картинки не збіглися. Введіть нові.",
        });
      }
      if (password !== "admin123") {
        return json(403, {
          field: "password",
          error: "Невірний пароль.",
        });
      }
      state.isLoggedIn = true;
      return json(200, {});
    }

    if (pathname === "/admin/api/devices" && method === "GET") {
      if (!state.isLoggedIn) {
        return json(401, { login: true });
      }
      return json(200, state.devices);
    }

    const waitMatch = pathname.match(/^\/admin\/devices\/([^/]+)\/waiting$/);
    if (waitMatch && method === "POST") {
      if (!state.isLoggedIn) return json(401, { login: true });
      const devId = waitMatch[1];
      if (state.failWaitingOnce) {
        state.failWaitingOnce = false;
        return json(500, { error: "Не вдалося зберегти налаштування" });
      }
      const postData = route.request().postData() || "";
      const params = new URLSearchParams(postData);
      const on = params.get("on") === "1";
      const dev = state.devices.find(d => d.id === devId);
      if (dev) dev.waiting = on;
      return json(200, {});
    }

    const openMatch = pathname.match(/^\/admin\/devices\/([^/]+)\/open$/);
    if (openMatch && method === "POST") {
      if (!state.isLoggedIn) return json(401, { login: true });
      const devId = openMatch[1];
      const dev = state.devices.find(d => d.id === devId);
      if (dev) {
        if (!dev.online) dev.requested = true;
        return json(200, { online: dev.online });
      }
      return json(404, { error: "Такого пристрою вже немає." });
    }

    const viewMatch = pathname.match(/^\/admin\/devices\/([^/]+)\/view$/);
    if (viewMatch && method === "POST") {
      if (!state.isLoggedIn) return json(401, { login: true });
      const devId = viewMatch[1];
      const postData = route.request().postData() || "";
      const params = new URLSearchParams(postData);
      const view = params.get("view");
      state.lastPostedView = { devId, view };
      state.viewPostCount++;
      const dev = state.devices.find(d => d.id === devId);
      if (dev) dev.view = view;
      return route.fulfill({ status: 200, body: "ok" });
    }

    const endMatch = pathname.match(/^\/admin\/devices\/([^/]+)\/end$/);
    if (endMatch && method === "POST") {
      if (!state.isLoggedIn) return json(401, { login: true });
      const devId = endMatch[1];
      const dev = state.devices.find(d => d.id === devId);
      if (dev) {
        dev.online = false;
        dev.requested = false;
        dev.status = "alive";
      }
      return json(200, {});
    }

    const forgetMatch = pathname.match(/^\/admin\/devices\/([^/]+)\/forget$/);
    if (forgetMatch && method === "POST") {
      if (!state.isLoggedIn) return json(401, { login: true });
      const devId = forgetMatch[1];
      state.devices = state.devices.filter(d => d.id !== devId);
      return json(200, {});
    }

    return route.fulfill({ status: 404, body: "Not found" });
  });
}

async function run() {
  fs.mkdirSync(OUT, { recursive: true });
  const browser = await chromium.launch({ executablePath: exe });
  const context = await browser.newContext({
    viewport: { width: 1024, height: 768 },
    deviceScaleFactor: 2,
  });
  const page = await context.newPage();

  const errors = [];
  page.on("pageerror", err => {
    errors.push(err.message);
    console.error("PAGE ERROR:", err);
  });

  const state = createMockServer();
  await setupRoutes(page, state);

  console.log("1. Navigating to /admin...");
  await page.goto("http://localhost:8080/admin");

  // Wait for login form to load
  await page.waitForSelector(".admin-card");
  await page.waitForSelector("input[type=password]");
  await page.screenshot({ path: path.join(OUT, "01-login-initial.png") });

  console.log("2. Submitting wrong captcha...");
  await page.fill("input[type=password]", "admin123");
  const inputs = await page.$$("input.op-field-input");
  await inputs[1].fill("00000");

  const loginBtn = await page.waitForSelector("button:has-text('Увійти')");
  await loginBtn.click();

  // Verify error banner appears
  await page.waitForSelector(".op-banner");
  const bannerText = await page.$eval(".op-banner", el => el.textContent);
  if (!bannerText.includes("Циفري з картинки не збіглися")) {
    throw new Error(`Expected captcha error, got: ${bannerText}`);
  }
  await page.screenshot({ path: path.join(OUT, "02-login-captcha-error.png") });

  console.log("3. Submitting correct captcha and logging in...");
  await inputs[1].fill("12345");
  await loginBtn.click();

  // Wait for devices list to load: 3 devices with session, alive, offline
  await page.waitForSelector(".admin-dev-card");
  await page.waitForFunction(() => document.querySelectorAll(".admin-dev-card").length === 3);

  // Check badges for each status
  const badges = await page.$$eval(".admin-devhead-badges .op-badge", els => els.map(e => e.textContent.trim()));
  console.log("Detected badges:", badges);
  if (!badges.some(b => b.includes("з'єднано"))) {
    throw new Error("Missing 'з'єднано' status badge");
  }
  if (!badges.some(b => b.includes("на зв'язку"))) {
    throw new Error("Missing 'на зв'язку' status badge");
  }
  if (!badges.some(b => b.includes("не на зв'язку"))) {
    throw new Error("Missing 'не на зв'язку' status badge");
  }

  await page.screenshot({ path: path.join(OUT, "03-devices-light-wide.png") });

  console.log("4. Toggling theme to dark mode...");
  const themeToggle = await page.waitForSelector("button.op-theme-toggle");
  await themeToggle.click();
  await page.waitForTimeout(300);
  await page.screenshot({ path: path.join(OUT, "04-devices-dark-wide.png") });

  console.log("5. Testing narrow viewport...");
  await page.setViewportSize({ width: 440, height: 900 });
  await page.waitForTimeout(300);
  await page.screenshot({ path: path.join(OUT, "05-devices-dark-narrow.png") });

  // Restore wide viewport
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.waitForTimeout(300);

  console.log("6. Selecting non-online device ('Офіс', status: alive)...");
  const devCards = await page.$$(".admin-dev-card");
  // devCards[1] is 'Офіс'
  await devCards[1].click();

  // Verify empty state appears on the right
  await page.waitForSelector(".admin-waiting-area .op-empty");
  const emptyStateText = await page.$eval(".admin-waiting-area .op-empty", el => el.textContent);
  if (!emptyStateText.includes("Сесію запитано") || !emptyStateText.includes("Чекаю пристрій")) {
    throw new Error(`Expected waiting empty state, got: ${emptyStateText}`);
  }
  // Verify 'запитано' badge appears on device 2
  await page.waitForFunction(() => {
    const text = document.querySelectorAll(".admin-dev-card")[1].textContent;
    return text.includes("запитано");
  });
  await page.screenshot({ path: path.join(OUT, "06-device-not-online-empty-state.png") });

  console.log("7. Selecting online device ('Дім', status: session)...");
  // devCards[0] is 'Дім'
  await devCards[0].click();

  // Verify iframe is shown
  await page.waitForSelector("iframe.admin-iframe");
  const iframeHandle = await page.$("iframe.admin-iframe");
  const frame = await iframeHandle.contentFrame();
  await frame.waitForSelector("#current-view");
  const initialView = await frame.$eval("#current-view", el => el.textContent);
  if (initialView !== "login") {
    throw new Error(`Expected initial view 'login', got: ${initialView}`);
  }
  await page.screenshot({ path: path.join(OUT, "07-device-online-panel.png") });

  console.log("8. Testing view sync: Admin to Server (iframe clicks Wi-Fi)...");
  state.lastPostedView = null;
  state.viewPostCount = 0;
  await frame.click("#btn-wifi");

  // Verify show was called in iframe and view was posted to server
  await frame.waitForFunction(() => document.getElementById("current-view").textContent === "wifi");
  const postStart = Date.now();
  while (!state.lastPostedView && Date.now() - postStart < 5000) {
    await page.waitForTimeout(50);
  }
  if (!state.lastPostedView) {
    throw new Error("Timed out waiting for view POST from iframe wrapper");
  }
  if (state.lastPostedView.view !== "wifi" || state.lastPostedView.devId !== "0123456789abcdef") {
    throw new Error(`Unexpected posted view: ${JSON.stringify(state.lastPostedView)}`);
  }
  console.log("View posted to server successfully:", state.lastPostedView);

  console.log("9. Testing view sync: Server to Admin without echo (server switches to 'bms')...");
  state.lastPostedView = null;
  const postCountBefore = state.viewPostCount;
  // Change device 1 view on server
  state.devices[0].view = "bms";

  // Wait for polled update in iframe
  await frame.waitForFunction(() => document.getElementById("current-view").textContent === "bms", null, { timeout: 6000 });
  // Ensure no extra POST was triggered (no echo)
  if (state.viewPostCount !== postCountBefore) {
    throw new Error("Server-to-admin view sync echoed back to server!");
  }
  await page.screenshot({ path: path.join(OUT, "08-view-sync-bms.png") });

  console.log("10. Testing narrow viewport with panel active...");
  await page.setViewportSize({ width: 440, height: 900 });
  await page.waitForTimeout(300);
  await page.screenshot({ path: path.join(OUT, "09-panel-narrow.png") });

  // Restore wide viewport
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.waitForTimeout(300);

  console.log("11. Toggling 'Чекаю пристрій' switch (success)...");
  const switches = await page.$$("button.op-switch");
  await switches[0].click();
  await page.waitForFunction(() => document.querySelectorAll("button.op-switch")[0].getAttribute("aria-checked") === "true", null, { timeout: 5000 });
  await page.screenshot({ path: path.join(OUT, "10-waiting-on.png") });

  console.log("12. Toggling 'Чекаю пристрій' switch (simulated error)...");
  state.failWaitingOnce = true;
  await switches[1].click();
  await page.waitForSelector(".admin-card-msg .op-banner", { timeout: 5000 });
  await page.screenshot({ path: path.join(OUT, "11-waiting-error.png") });

  console.log("13. Ending session on device 1 ('Дім')...");
  const endBtn = await page.waitForSelector("button:has-text('Завершити сесію')");
  await endBtn.click();

  // Verify empty state appears after session ends
  await page.waitForSelector(".admin-waiting-area .op-empty", { timeout: 5000 });
  await page.screenshot({ path: path.join(OUT, "12-session-ended.png") });

  console.log("14. Opening forget dialog for device 2 ('Офіс')...");
  const forgetButtons = await page.$$("button:has-text('Забути')");
  await forgetButtons[1].click();

  await page.waitForSelector(".op-dialog", { timeout: 5000 });
  const dialogMsg = await page.$eval(".op-dialog-message", el => el.textContent);
  if (!dialogMsg.includes("«Офіс» зникне зі списку")) {
    throw new Error(`Unexpected dialog message: ${dialogMsg}`);
  }
  await page.screenshot({ path: path.join(OUT, "13-forget-dialog.png") });

  console.log("15. Confirming forget in dialog...");
  const confirmBtn = await page.waitForSelector(".op-dialog-actions button.op-danger:has-text('Забути')");
  await confirmBtn.click();
  await page.waitForSelector(".op-dialog", { state: "detached" });

  // Wait for dialog to close and list to have 2 devices
  await page.waitForFunction(() => document.querySelectorAll(".admin-dev-card").length === 2, null, { timeout: 5000 });
  await page.screenshot({ path: path.join(OUT, "14-two-devices-left.png") });

  console.log("16. Forgetting all remaining devices to show empty state...");
  while ((await page.$$(".admin-dev-card button:has-text('Забути')")).length > 0) {
    const btn = await page.waitForSelector(".admin-dev-card button:has-text('Забути')");
    await btn.click();
    await page.waitForSelector(".op-dialog");
    const conf = await page.waitForSelector(".op-dialog-actions button.op-danger:has-text('Забути')");
    await conf.click();
    await page.waitForSelector(".op-dialog", { state: "detached" });
    await page.waitForTimeout(200);
  }

  // Wait for empty state in both panels
  await page.waitForSelector(".admin-devices-empty .op-empty", { timeout: 5000 });
  const emptyTitle = await page.$eval(".admin-devices-empty .op-empty-title", el => el.textContent);
  if (!emptyTitle.includes("Пристроїв ще немає")) {
    throw new Error(`Unexpected empty title: ${emptyTitle}`);
  }
  await page.screenshot({ path: path.join(OUT, "15-empty-state.png") });

  await browser.close();

  const passed = errors.length === 0;
  if (passed) {
    console.log("PASS admin-ui");
  } else {
    console.error("FAIL admin-ui", errors);
    process.exitCode = 1;
  }
}

run().catch(err => {
  console.error("FAIL admin-ui error:", err);
  process.exit(1);
});
