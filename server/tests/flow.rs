//! The server driven through its router: a fake device on one side, the
//! owner's browser on the other.

use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderMap, Request, StatusCode};
use remote_server::{App, Config};
use serde_json::Value;
use tower::ServiceExt;

const ID: &str = "0123456789abcdef";
const SECRET: &str = "00112233445566778899aabbccddeeff";

fn app() -> App {
    App::new(Config {
        admin_password: "correct horse battery".into(),
        data: None,
    })
}

async fn call(app: &App, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.clone().router().oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    (
        parts.status,
        parts.headers,
        to_bytes(body, usize::MAX).await.unwrap().to_vec(),
    )
}

fn device(method: &str, uri: &str, secret: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", ID)
        .header("x-device-secret", secret)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body.into())
        .unwrap()
}

async fn checkin(app: &App) -> Value {
    let (status, _, body) = call(
        app,
        device(
            "POST",
            "/device/checkin",
            SECRET,
            "name=%D0%94%D1%96%D0%BC&version=0.1.0",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).unwrap()
}

fn login(form: &str) -> Request<Body> {
    Request::post("/admin/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_string()))
        .unwrap()
}

/// A captcha for the login form: its id and, peeked from the server, the answer.
async fn captcha(app: &App) -> (String, String) {
    let (_, _, body) = call(
        app,
        Request::get("/admin/api/captcha")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let id = json(&body)["id"].as_str().unwrap().to_string();
    let answer = app.captcha_answer(&id).unwrap();
    (id, answer)
}

/// Logs in and returns the cookie header for a browser that picked the device.
async fn admin(app: &App) -> String {
    let (id, answer) = captcha(app).await;
    let (_, headers, _) = call(
        app,
        login(&format!(
            "password=correct+horse+battery&captcha_id={id}&captcha={answer}"
        )),
    )
    .await;
    let set = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    let token = set.split(';').next().unwrap().to_string();
    format!("{token}; svitlo_device={ID}; session=panel-token")
}

fn json(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap()
}

fn browser(method: &str, uri: &str, cookies: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookies)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body.into())
        .unwrap()
}

#[tokio::test]
async fn first_contact_registers_and_the_secret_must_match_after() {
    let app = app();
    assert_eq!(
        checkin(&app).await,
        serde_json::json!({ "session": false, "soon": false })
    );
    let (status, _, _) = call(&app, device("POST", "/device/checkin", &"f".repeat(32), "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&app, device("POST", "/device/checkin", "short", "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// Paused time: the long poll below runs out without waiting 25 real seconds.
#[tokio::test(start_paused = true)]
async fn waiting_flag_brings_the_device_back_soon_and_open_asks_for_a_session() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;
    call(
        &app,
        browser(
            "POST",
            &format!("/admin/devices/{ID}/waiting"),
            &cookies,
            "on=1",
        ),
    )
    .await;
    assert_eq!(
        checkin(&app).await,
        serde_json::json!({ "session": false, "soon": true })
    );

    let (status, headers, body) = call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/open"), &cookies, ""),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body), serde_json::json!({ "online": false }));
    assert!(headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .starts_with(&format!("svitlo_device={ID};")));
    assert_eq!(checkin(&app).await["session"], true);

    // Once the session starts the request is used up.
    let (status, _, _) = call(&app, device("GET", "/device/next", SECRET, "")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(checkin(&app).await["session"], false);
}

#[tokio::test]
async fn browser_requests_reach_the_panel_and_come_back() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;

    let device_side = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            let (_, _, body) = call(
                &app,
                device("GET", &format!("/device/body/{rid}"), SECRET, ""),
            )
            .await;
            // Binary answers survive, blank lines inside included.
            let mut reply =
                b"content-type: application/octet-stream\nset-cookie: session=new; Path=/\n\n"
                    .to_vec();
            reply.extend_from_slice(&[0, 10, 10, 255]);
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "201".parse().unwrap());
            call(&app, request).await;
            (job, body)
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, headers, body) = call(
        &app,
        browser("POST", "/api/messages?x=1", &cookies, "device_name=a"),
    )
    .await;
    let (job, relayed_body) = device_side.await.unwrap();
    assert_eq!(job["method"], "POST");
    assert_eq!(job["path"], "/api/messages?x=1");
    assert_eq!(
        job["cookie"],
        Value::Null,
        "browser cookies are ignored, server has no panel session yet"
    );
    assert_eq!(job["content_type"], "application/x-www-form-urlencoded");
    assert_eq!(job["body_len"], 13);
    assert_eq!(relayed_body, b"device_name=a");
    assert_eq!(status, StatusCode::CREATED);
    assert!(
        headers.get(header::SET_COOKIE).is_none(),
        "panel Set-Cookie never reaches the browser"
    );
    assert_eq!(headers[header::CONTENT_TYPE], "application/octet-stream");
    assert_eq!(body, [0, 10, 10, 255]);

    // Next request carries the stored session token
    let device_side2 = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            let reply = b"content-type: text/plain\n\nok".to_vec();
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "200".parse().unwrap());
            call(&app, request).await;
            job
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _, _) = call(&app, browser("GET", "/api/state", &cookies, "")).await;
    assert_eq!(status, StatusCode::OK);
    let job2 = device_side2.await.unwrap();
    assert_eq!(job2["cookie"], "session=new");
}

#[tokio::test]
async fn the_relay_needs_the_admin() {
    let app = app();
    checkin(&app).await;
    let (status, headers, _) = call(
        &app,
        browser("GET", "/api/state", &format!("svitlo_device={ID}"), ""),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/admin");
    let (id, answer) = captcha(&app).await;
    let (status, headers, body) = call(
        &app,
        login(&format!("password=guess&captcha_id={id}&captcha={answer}")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert_eq!(json(&body)["field"], "password");
}

// Paused time: failed logins wait a second.
#[tokio::test(start_paused = true)]
async fn the_password_alone_does_not_log_in() {
    let app = app();
    let (id, answer) = captcha(&app).await;
    let (status, headers, body) = call(&app, login("password=correct+horse+battery")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert_eq!(json(&body)["field"], "captcha");

    let wrong = if answer == "00000" { "11111" } else { "00000" };
    let (_, headers, _) = call(
        &app,
        login(&format!(
            "password=correct+horse+battery&captcha_id={id}&captcha={wrong}"
        )),
    )
    .await;
    assert!(headers.get(header::SET_COOKIE).is_none());
    // A wrong guess uses the captcha up.
    let (_, headers, _) = call(
        &app,
        login(&format!(
            "password=correct+horse+battery&captcha_id={id}&captcha={answer}"
        )),
    )
    .await;
    assert!(headers.get(header::SET_COOKIE).is_none());
}

#[tokio::test(start_paused = true)]
async fn a_solved_captcha_logs_in_once() {
    let app = app();
    let (id, answer) = captcha(&app).await;
    let form = format!("password=correct+horse+battery&captcha_id={id}&captcha={answer}");
    let (status, headers, _) = call(&app, login(&form)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(header::SET_COOKIE).is_some());
    let (_, headers, _) = call(&app, login(&form)).await;
    assert!(headers.get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn the_captcha_is_a_picture_without_text() {
    let app = app();
    let (id, _) = captcha(&app).await;
    let (status, headers, body) = call(
        &app,
        Request::get(format!("/admin/captcha/{id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/svg+xml");
    let svg = String::from_utf8(body).unwrap();
    assert!(svg.starts_with("<svg") && svg.contains("<path"));
    assert!(!svg.contains("<text"));
    let (status, _, _) = call(
        &app,
        Request::get(format!("/admin/captcha/{}", "0".repeat(32)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_device_out_of_session_is_asked_for_one() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;
    let (status, headers, _) = call(&app, browser("GET", "/api/state", &cookies, "")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], format!("/admin?open={ID}"));
}

#[tokio::test]
async fn ending_the_session_tells_the_device() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;
    let waiting = {
        let app = app.clone();
        tokio::spawn(async move {
            call(&app, device("GET", "/device/next", SECRET, ""))
                .await
                .0
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/end"), &cookies, ""),
    )
    .await;
    assert_eq!(waiting.await.unwrap(), StatusCode::GONE);
}

#[tokio::test]
async fn admin_and_assets_are_served_with_correct_content_types() {
    let app = app();
    let (status, headers, body) =
        call(&app, Request::get("/admin").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains("/admin/admin.js"));
    assert!(html.contains("/admin/admin_bg.wasm"));

    let (status, headers, _) =
        call(&app, Request::get("/admin/").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");

    let (status, headers, js) = call(
        &app,
        Request::get("/admin/admin.js").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/javascript");
    assert!(!js.is_empty());

    let (status, headers, wasm) = call(
        &app,
        Request::get("/admin/admin_bg.wasm")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/wasm");
    assert!(wasm.starts_with(b"\0asm"));

    let (status, headers, wasm2) = call(
        &app,
        Request::get("/admin/admin.wasm")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/wasm");
    assert!(wasm2.starts_with(b"\0asm"));

    let (status, headers, css) = call(
        &app,
        Request::get("/admin/admin.css")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/css; charset=utf-8");
    assert!(!css.is_empty());

    let (status, headers, font) = call(
        &app,
        Request::get("/admin/assets/fonts/CascadiaCode.ttf")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "font/ttf");
    assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=86400");
    assert!(!font.is_empty());

    let (status, headers, ofl) = call(
        &app,
        Request::get("/admin/assets/fonts/CascadiaCode-OFL.txt")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/plain; charset=utf-8");
    assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=86400");
    assert!(!ofl.is_empty());

    for (path, cache) in [
        ("/admin", "no-store"),
        ("/admin/index.html", "no-store"),
        ("/admin/admin.js", "public, max-age=3600"),
        ("/admin/admin_bg.wasm", "public, max-age=3600"),
        ("/admin/admin.css", "public, max-age=3600"),
    ] {
        let (status, headers, _) =
            call(&app, Request::get(path).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(headers[header::CACHE_CONTROL], cache, "{path}");
    }
}

#[tokio::test]
async fn the_device_list_needs_the_admin_and_follows_the_actions() {
    let app = app();
    checkin(&app).await;
    let (status, _, body) = call(&app, browser("GET", "/admin/api/devices", "", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json(&body), serde_json::json!({ "login": true }));
    let (status, _, _) = call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/waiting"), "", "on=1"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let cookies = admin(&app).await;
    let list = |app: App, cookies: String| async move {
        json(
            &call(&app, browser("GET", "/admin/api/devices", &cookies, ""))
                .await
                .2,
        )
    };
    let devices = list(app.clone(), cookies.clone()).await;
    assert_eq!(devices[0]["id"], ID);
    assert_eq!(devices[0]["name"], "Дім");
    assert_eq!(devices[0]["version"], "0.1.0");
    assert_eq!(devices[0]["online"], false);
    assert_eq!(devices[0]["requested"], false);
    assert_eq!(devices[0]["waiting"], false);

    let (status, _, _) = call(
        &app,
        browser(
            "POST",
            &format!("/admin/devices/{ID}/waiting"),
            &cookies,
            "on=1",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/open"), &cookies, ""),
    )
    .await;
    let devices = list(app.clone(), cookies.clone()).await;
    assert_eq!(devices[0]["waiting"], true);
    assert_eq!(devices[0]["requested"], true);

    let (status, _, _) = call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/forget"), &cookies, ""),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list(app.clone(), cookies.clone()).await,
        serde_json::json!([])
    );
    let (status, _, _) = call(
        &app,
        browser(
            "POST",
            &format!("/admin/devices/{ID}/waiting"),
            &cookies,
            "on=1",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn after_ending_the_session_the_browser_goes_back_to_the_admin_at_once() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;
    let waiting = {
        let app = app.clone();
        tokio::spawn(async move { call(&app, device("GET", "/device/next", SECRET, "")).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/end"), &cookies, ""),
    )
    .await;
    waiting.await.unwrap();
    let (status, headers, _) = tokio::time::timeout(
        Duration::from_secs(2),
        call(&app, browser("GET", "/", &cookies, "")),
    )
    .await
    .expect("the page answers instead of waiting for the device");
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], format!("/admin?open={ID}"));
}

// Paused time: the device stops polling, as it does while it scans for a BMS.
#[tokio::test(start_paused = true)]
async fn a_request_the_busy_device_never_takes_fails_fast_and_is_dropped() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;
    let (status, _, _) = call(&app, device("GET", "/device/next", SECRET, "")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let started = tokio::time::Instant::now();
    let (status, _, _) = call(&app, browser("GET", "/api/state", &cookies, "")).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "gave up after {:?}",
        started.elapsed()
    );

    let (status, _, _) = call(&app, device("GET", "/device/next", SECRET, "")).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the browser stopped waiting, so the device is not asked"
    );
}

#[tokio::test]
async fn no_longer_waiting_for_the_device_drops_the_session_request() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;
    let post = |path: &str, body: &'static str| {
        browser(
            "POST",
            &format!("/admin/devices/{ID}/{path}"),
            &cookies,
            body,
        )
    };
    call(&app, post("waiting", "on=1")).await;
    call(&app, post("open", "")).await;
    call(&app, post("waiting", "on=0")).await;

    let (_, _, body) = call(&app, browser("GET", "/admin/api/devices", &cookies, "")).await;
    assert_eq!(json(&body)[0]["requested"], false);
    assert_eq!(checkin(&app).await["session"], false);
}

#[tokio::test(start_paused = true)]
async fn device_status_and_view_in_device_list() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;

    // Initially checked in, no poll: status is "alive", view is null.
    let (_, _, body) = call(&app, browser("GET", "/admin/api/devices", &cookies, "")).await;
    let list = json(&body);
    assert_eq!(list[0]["status"], "alive");
    assert_eq!(list[0]["view"], Value::Null);

    // Device starts polling: status becomes "session".
    let (status, _, _) = call(&app, device("GET", "/device/next", SECRET, "")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, body) = call(&app, browser("GET", "/admin/api/devices", &cookies, "")).await;
    let list = json(&body);
    assert_eq!(list[0]["status"], "session");
}

#[tokio::test]
async fn device_view_endpoint_validation_and_reset() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;

    // Needs admin
    let (status, _, _) = call(
        &app,
        browser(
            "POST",
            &format!("/admin/devices/{ID}/view"),
            "",
            "view=login",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Invalid view value
    let (status, _, _) = call(
        &app,
        browser(
            "POST",
            &format!("/admin/devices/{ID}/view"),
            &cookies,
            "view=invalid",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unknown device
    let (status, _, _) = call(
        &app,
        browser(
            "POST",
            "/admin/devices/nonexistent/view",
            &cookies,
            "view=login",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Valid views: login, wifi, move, bot, dashboard, bms
    for v in ["login", "wifi", "move", "bot", "dashboard", "bms"] {
        let (status, _, _) = call(
            &app,
            browser(
                "POST",
                &format!("/admin/devices/{ID}/view"),
                &cookies,
                format!("view={v}"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, _, body) = call(&app, browser("GET", "/admin/api/devices", &cookies, "")).await;
        assert_eq!(json(&body)[0]["view"], v);
    }

    // Ending the session resets the view to null
    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/end"), &cookies, ""),
    )
    .await;
    let (_, _, body) = call(&app, browser("GET", "/admin/api/devices", &cookies, "")).await;
    assert_eq!(json(&body)[0]["view"], Value::Null);
}

#[tokio::test]
async fn panel_cookie_is_stored_on_server_and_not_sent_to_browser() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;

    // Start session
    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/open"), &cookies, ""),
    )
    .await;

    // Fake device answers first request (login) and sets session cookie
    let device_task = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            // First call has no stored session cookie
            assert_eq!(job["cookie"], Value::Null);

            let reply = b"content-type: text/plain\nset-cookie: session=secret123; Path=/; HttpOnly; SameSite=Strict\n\nok".to_vec();
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "200".parse().unwrap());
            call(&app, request).await;
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, headers, body) =
        call(&app, browser("POST", "/api/login", &cookies, "pass=123")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"ok");
    // Set-Cookie: session=... must NOT be passed to the browser!
    assert!(headers.get(header::SET_COOKIE).is_none());
    device_task.await.unwrap();

    // Next request to device: browser sends a fake session cookie, but server sends its stored token!
    let device_task2 = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            // Must carry stored token "session=secret123"
            assert_eq!(job["cookie"], "session=secret123");

            // Logout reply
            let reply = b"content-type: text/plain\nset-cookie: session=; Path=/; Max-Age=0\n\nbye"
                .to_vec();
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "200".parse().unwrap());
            call(&app, request).await;
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, headers, body) = call(&app, browser("POST", "/api/logout", &cookies, "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"bye");
    assert!(headers.get(header::SET_COOKIE).is_none());
    device_task2.await.unwrap();

    // After logout, next request carries no cookie
    let device_task3 = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            assert_eq!(job["cookie"], Value::Null);

            let reply = b"content-type: text/plain\n\nstate".to_vec();
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "200".parse().unwrap());
            call(&app, request).await;
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _, _) = call(&app, browser("GET", "/api/state", &cookies, "")).await;
    assert_eq!(status, StatusCode::OK);
    device_task3.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn coalesced_get_requests_send_single_job_to_device() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;

    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/open"), &cookies, ""),
    )
    .await;

    // Fake device receives only ONE job and answers it
    let device_task = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            assert_eq!(job["path"], "/api/state");

            // Delay reply slightly so both browsers are concurrently in-flight
            tokio::time::sleep(Duration::from_millis(50)).await;

            let reply = b"content-type: application/json\n\n{\"voltage\":230}".to_vec();
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "200".parse().unwrap());
            call(&app, request).await;
        })
    };

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Two browsers GET /api/state at once
    let b1 = {
        let app = app.clone();
        let cookies = cookies.clone();
        tokio::spawn(async move { call(&app, browser("GET", "/api/state", &cookies, "")).await })
    };
    let b2 = {
        let app = app.clone();
        let cookies = cookies.clone();
        tokio::spawn(async move { call(&app, browser("GET", "/api/state", &cookies, "")).await })
    };

    let (res1, res2) = tokio::join!(b1, b2);
    let (s1, _, body1) = res1.unwrap();
    let (s2, _, body2) = res2.unwrap();

    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(body1, b"{\"voltage\":230}");
    assert_eq!(body2, b"{\"voltage\":230}");
    device_task.await.unwrap();

    // Confirm device has no more pending jobs: poll returns NO_CONTENT after timeout
    let (status, _, _) = call(&app, device("GET", "/device/next", SECRET, "")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test(start_paused = true)]
async fn cached_get_requests_served_without_device_poll_and_post_invalidates() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;

    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/open"), &cookies, ""),
    )
    .await;

    // Device serves first GET
    let device_task = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            let reply = b"content-type: application/json\n\n{\"c\":1}".to_vec();
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "200".parse().unwrap());
            call(&app, request).await;
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (s, _, body) = call(&app, browser("GET", "/api/state", &cookies, "")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, b"{\"c\":1}");
    device_task.await.unwrap();

    // Within 1.5s (say 500ms later), next GET should be served from cache without device poll
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (s, _, body) = call(&app, browser("GET", "/api/state", &cookies, "")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, b"{\"c\":1}");

    // POST request arrives: goes to device, and invalidates cache
    let device_post = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            assert_eq!(job["method"], "POST");
            let reply = b"content-type: text/plain\n\nok".to_vec();
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "200".parse().unwrap());
            call(&app, request).await;
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (s, _, body) = call(&app, browser("POST", "/api/switch", &cookies, "on=1")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, b"ok");
    device_post.await.unwrap();

    // Now GET /api/state should NOT come from cache even though < 1.5s since last GET, because POST invalidated it!
    let device_get2 = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            let reply = b"content-type: application/json\n\n{\"c\":2}".to_vec();
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "200".parse().unwrap());
            call(&app, request).await;
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (s, _, body) = call(&app, browser("GET", "/api/state", &cookies, "")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, b"{\"c\":2}");
    device_get2.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn gateway_timeout_is_not_cached_and_next_request_goes_to_device() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;

    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/open"), &cookies, ""),
    )
    .await;

    // Device starts polling to be online
    let (status, _, _) = call(&app, device("GET", "/device/next", SECRET, "")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Browser requests GET /api/state, but device does not pick it up -> 504 Gateway Timeout
    let (s, _, _) = call(&app, browser("GET", "/api/state", &cookies, "")).await;
    assert_eq!(s, StatusCode::GATEWAY_TIMEOUT);

    // Immediately after 504, next GET /api/state must NOT return cached 504; it must go to device!
    let device_task = {
        let app = app.clone();
        tokio::spawn(async move {
            let (status, _, job) = call(&app, device("GET", "/device/next", SECRET, "")).await;
            assert_eq!(status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&job).unwrap();
            let rid = job["rid"].as_str().unwrap().to_string();
            let reply = b"content-type: application/json\n\n{\"fresh\":true}".to_vec();
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request
                .headers_mut()
                .insert("x-status", "200".parse().unwrap());
            call(&app, request).await;
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (s, _, body) = call(&app, browser("GET", "/api/state", &cookies, "")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, b"{\"fresh\":true}");
    device_task.await.unwrap();
}

#[tokio::test]
async fn ending_session_releases_coalesced_browsers_at_once() {
    let app = app();
    checkin(&app).await;
    let cookies = admin(&app).await;

    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/open"), &cookies, ""),
    )
    .await;

    // Start poll to be online
    let _device_poll = {
        let app = app.clone();
        tokio::spawn(async move { call(&app, device("GET", "/device/next", SECRET, "")).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Two browsers GET /api/state at once
    let b1 = {
        let app = app.clone();
        let cookies = cookies.clone();
        tokio::spawn(async move { call(&app, browser("GET", "/api/state", &cookies, "")).await })
    };
    let b2 = {
        let app = app.clone();
        let cookies = cookies.clone();
        tokio::spawn(async move { call(&app, browser("GET", "/api/state", &cookies, "")).await })
    };

    tokio::time::sleep(Duration::from_millis(50)).await;

    // End session
    call(
        &app,
        browser("POST", &format!("/admin/devices/{ID}/end"), &cookies, ""),
    )
    .await;

    let (res1, res2) = tokio::join!(b1, b2);
    let (s1, h1, _) = res1.unwrap();
    let (s2, h2, _) = res2.unwrap();

    assert_eq!(s1, StatusCode::SEE_OTHER);
    assert_eq!(h1[header::LOCATION], format!("/admin?open={ID}"));
    assert_eq!(s2, StatusCode::SEE_OTHER);
    assert_eq!(h2[header::LOCATION], format!("/admin?open={ID}"));

    // Next poll by device learns the session ended
    let (status, _, _) = call(&app, device("GET", "/device/next", SECRET, "")).await;
    assert_eq!(status, StatusCode::GONE);
}
