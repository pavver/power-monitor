// Drives server/admin in headless Chromium against a fake JSON API.
// Usage: node server/admin/ui-check.js
const { chromium } = require(process.env.PLAYWRIGHT_CORE || "/tmp/claude-1000/-home-pavver-rust-esp/5bfd559e-822e-4e5e-941a-e07d256d5099/scratchpad/pw/node_modules/playwright-core");
const fs = require("fs");
const path = require("path");

const DIST = path.resolve(__dirname, "dist");
const OUT = process.argv[2] || "/tmp/claude-1000/-home-pavver-rust-esp/5bfd559e-822e-4e5e-941a-e07d256d5099/scratchpad/admin-shots";

const exe = fs.readdirSync(path.join(process.env.HOME, ".cache/ms-playwright/chromium_headless_shell-1228"))
  .filter(d => d.startsWith("chrome"))
  .map(d => path.join(process.env.HOME, ".cache/ms-playwright/chromium_headless_shell-1228", d, "chrome-headless-shell"))[0];

function createMockServer() {
  return {
    isLoggedIn: false,
    captchaCounter: 0,
    failWaitingOnce: false,
    devices: [
      {
        id: "0123456789abcdef",
        name: "Дім",
        version: "0.1.0",
        seen: 15,
        online: true,
        requested: false,
        waiting: false,
      },
      {
        id: "fedcba9876543210",
        name: "Офіс",
        version: "0.2.0",
        seen: 320,
        online: false,
        requested: false,
        waiting: false,
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
          error: "Цифри з картинки не збіглися. Введіть нові.",
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
      if (dev) dev.requested = true;
      return json(200, { online: false });
    }

    const endMatch = pathname.match(/^\/admin\/devices\/([^/]+)\/end$/);
    if (endMatch && method === "POST") {
      if (!state.isLoggedIn) return json(401, { login: true });
      const devId = endMatch[1];
      const dev = state.devices.find(d => d.id === devId);
      if (dev) {
        dev.online = false;
        dev.requested = false;
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
    viewport: { width: 440, height: 900 },
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
  // Find captcha input (the text input after password)
  const inputs = await page.$$("input.op-field-input");
  // inputs[0] is password, inputs[1] is captcha
  await inputs[1].fill("00000");

  const loginBtn = await page.waitForSelector("button:has-text('Увійти')");
  await loginBtn.click();

  // Verify error banner appears
  await page.waitForSelector(".op-banner");
  const bannerText = await page.$eval(".op-banner", el => el.textContent);
  if (!bannerText.includes("Цифри з картинки не збіглися")) {
    throw new Error(`Expected captcha error, got: ${bannerText}`);
  }
  await page.screenshot({ path: path.join(OUT, "02-login-captcha-error.png") });

  console.log("3. Submitting correct captcha and logging in...");
  // New captcha has been issued, so 12345 will match
  await inputs[1].fill("12345");
  await loginBtn.click();

  // Wait for devices list to load
  await page.waitForSelector(".admin-devhead");
  await page.waitForFunction(() => document.querySelectorAll(".admin-devhead").length === 2);
  await page.screenshot({ path: path.join(OUT, "03-devices-light.png") });

  console.log("4. Toggling theme to dark mode...");
  const themeToggle = await page.waitForSelector("button.op-theme-toggle");
  await themeToggle.click();
  await page.waitForTimeout(300);
  await page.screenshot({ path: path.join(OUT, "04-devices-dark.png") });

  console.log("5. Toggling 'Чекаю пристрій' switch (success)...");
  const switches = await page.$$("button.op-switch");
  if (switches.length < 2) throw new Error("Expected at least 2 switches");
  await switches[0].click();
  await page.waitForFunction(() => document.querySelectorAll("button.op-switch")[0].getAttribute("aria-checked") === "true", null, { timeout: 5000 });
  await page.screenshot({ path: path.join(OUT, "05-waiting-on.png") });

  console.log("6. Toggling 'Чекаю пристрій' switch (simulated error)...");
  state.failWaitingOnce = true;
  await switches[1].click();
  // Wait for error banner on device 2
  await page.waitForSelector(".admin-card-msg .op-banner", { timeout: 5000 });
  await page.screenshot({ path: path.join(OUT, "06-waiting-error.png") });

  console.log("7. Opening forget dialog for device 2 ('Офіс')...");
  const forgetButtons = await page.$$("button:has-text('Забути')");
  await forgetButtons[1].click();

  // Wait for ConfirmDialog
  await page.waitForSelector(".op-dialog", { timeout: 5000 });
  const dialogMsg = await page.$eval(".op-dialog-message", el => el.textContent);
  if (!dialogMsg.includes("«Офіс» зникне зі списку")) {
    throw new Error(`Unexpected dialog message: ${dialogMsg}`);
  }
  await page.screenshot({ path: path.join(OUT, "07-forget-dialog.png") });

  console.log("8. Confirming forget in dialog...");
  const confirmBtn = await page.waitForSelector(".op-dialog-actions button.op-danger:has-text('Забути')");
  await confirmBtn.click();

  // Wait for dialog to close and list to have 1 device
  await page.waitForFunction(() => document.querySelectorAll(".admin-devhead").length === 1, null, { timeout: 5000 });
  await page.screenshot({ path: path.join(OUT, "08-one-device-left.png") });

  console.log("9. Forgetting remaining device to show empty state...");
  const lastForgetBtn = await page.waitForSelector("button:has-text('Забути')");
  await lastForgetBtn.click();
  await page.waitForSelector(".op-dialog");
  const lastConfirmBtn = await page.waitForSelector(".op-dialog-actions button.op-danger:has-text('Забути')");
  await lastConfirmBtn.click();

  // Wait for empty state
  await page.waitForSelector(".op-empty", { timeout: 5000 });
  const emptyTitle = await page.$eval(".op-empty-title", el => el.textContent);
  if (!emptyTitle.includes("Пристроїв ще немає")) {
    throw new Error(`Unexpected empty title: ${emptyTitle}`);
  }
  await page.screenshot({ path: path.join(OUT, "09-empty-state.png") });

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
