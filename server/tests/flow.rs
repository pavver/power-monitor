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
    App::new(Config { admin_password: "correct horse battery".into(), data: None })
}

async fn call(app: &App, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.clone().router().oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    (parts.status, parts.headers, to_bytes(body, usize::MAX).await.unwrap().to_vec())
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
    let (status, _, body) = call(app, device("POST", "/device/checkin", SECRET, "name=%D0%94%D1%96%D0%BC&version=0.1.0")).await;
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
    let (_, _, body) = call(app, Request::get("/admin/api/captcha").body(Body::empty()).unwrap()).await;
    let id = json(&body)["id"].as_str().unwrap().to_string();
    let answer = app.captcha_answer(&id).unwrap();
    (id, answer)
}

/// Logs in and returns the cookie header for a browser that picked the device.
async fn admin(app: &App) -> String {
    let (id, answer) = captcha(app).await;
    let (_, headers, _) = call(app, login(&format!("password=correct+horse+battery&captcha_id={id}&captcha={answer}"))).await;
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
    assert_eq!(checkin(&app).await, serde_json::json!({ "session": false, "soon": false }));
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
    call(&app, browser("POST", &format!("/admin/devices/{ID}/waiting"), &cookies, "on=1")).await;
    assert_eq!(checkin(&app).await, serde_json::json!({ "session": false, "soon": true }));

    let (status, headers, body) = call(&app, browser("POST", &format!("/admin/devices/{ID}/open"), &cookies, "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body), serde_json::json!({ "online": false }));
    assert!(headers[header::SET_COOKIE].to_str().unwrap().starts_with(&format!("svitlo_device={ID};")));
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
            let (_, _, body) = call(&app, device("GET", &format!("/device/body/{rid}"), SECRET, "")).await;
            // Binary answers survive, blank lines inside included.
            let mut reply = b"content-type: application/octet-stream\nset-cookie: session=new; Path=/\n\n".to_vec();
            reply.extend_from_slice(&[0, 10, 10, 255]);
            let mut request = device("POST", &format!("/device/reply/{rid}"), SECRET, reply);
            request.headers_mut().insert("x-status", "201".parse().unwrap());
            call(&app, request).await;
            (job, body)
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, headers, body) = call(&app, browser("POST", "/api/messages?x=1", &cookies, "device_name=a")).await;
    let (job, relayed_body) = device_side.await.unwrap();
    assert_eq!(job["method"], "POST");
    assert_eq!(job["path"], "/api/messages?x=1");
    assert_eq!(job["cookie"], "session=panel-token", "only the panel's cookie goes to the device");
    assert_eq!(job["content_type"], "application/x-www-form-urlencoded");
    assert_eq!(job["body_len"], 13);
    assert_eq!(relayed_body, b"device_name=a");
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(headers[header::SET_COOKIE], "session=new; Path=/");
    assert_eq!(headers[header::CONTENT_TYPE], "application/octet-stream");
    assert_eq!(body, [0, 10, 10, 255]);
}

#[tokio::test]
async fn the_relay_needs_the_admin() {
    let app = app();
    checkin(&app).await;
    let (status, headers, _) = call(&app, browser("GET", "/api/state", &format!("svitlo_device={ID}"), "")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/admin");
    let (id, answer) = captcha(&app).await;
    let (status, headers, body) = call(&app, login(&format!("password=guess&captcha_id={id}&captcha={answer}"))).await;
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
    let (_, headers, _) = call(&app, login(&format!("password=correct+horse+battery&captcha_id={id}&captcha={wrong}"))).await;
    assert!(headers.get(header::SET_COOKIE).is_none());
    // A wrong guess uses the captcha up.
    let (_, headers, _) = call(&app, login(&format!("password=correct+horse+battery&captcha_id={id}&captcha={answer}"))).await;
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
    let (status, headers, body) = call(&app, Request::get(format!("/admin/captcha/{id}")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/svg+xml");
    let svg = String::from_utf8(body).unwrap();
    assert!(svg.starts_with("<svg") && svg.contains("<path"));
    assert!(!svg.contains("<text"));
    let (status, _, _) = call(&app, Request::get(format!("/admin/captcha/{}", "0".repeat(32))).body(Body::empty()).unwrap()).await;
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
        tokio::spawn(async move { call(&app, device("GET", "/device/next", SECRET, "")).await.0 })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    call(&app, browser("POST", &format!("/admin/devices/{ID}/end"), &cookies, "")).await;
    assert_eq!(waiting.await.unwrap(), StatusCode::GONE);
}

#[tokio::test]
async fn pages_use_the_operator_font() {
    let app = app();
    let (_, _, page) = call(&app, Request::get("/admin").body(Body::empty()).unwrap()).await;
    assert!(String::from_utf8(page).unwrap().contains("url(/admin/operator-mono.woff)"));
    let (status, headers, font) = call(&app, Request::get("/admin/operator-mono.woff").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "font/woff");
    assert!(font.starts_with(b"wOFF"));
}

#[tokio::test]
async fn the_device_list_needs_the_admin_and_follows_the_actions() {
    let app = app();
    checkin(&app).await;
    let (status, _, body) = call(&app, browser("GET", "/admin/api/devices", "", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json(&body), serde_json::json!({ "login": true }));
    let (status, _, _) = call(&app, browser("POST", &format!("/admin/devices/{ID}/waiting"), "", "on=1")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let cookies = admin(&app).await;
    let list = |app: App, cookies: String| async move { json(&call(&app, browser("GET", "/admin/api/devices", &cookies, "")).await.2) };
    let devices = list(app.clone(), cookies.clone()).await;
    assert_eq!(devices[0]["id"], ID);
    assert_eq!(devices[0]["name"], "Дім");
    assert_eq!(devices[0]["version"], "0.1.0");
    assert_eq!(devices[0]["online"], false);
    assert_eq!(devices[0]["requested"], false);
    assert_eq!(devices[0]["waiting"], false);

    let (status, _, _) = call(&app, browser("POST", &format!("/admin/devices/{ID}/waiting"), &cookies, "on=1")).await;
    assert_eq!(status, StatusCode::OK);
    call(&app, browser("POST", &format!("/admin/devices/{ID}/open"), &cookies, "")).await;
    let devices = list(app.clone(), cookies.clone()).await;
    assert_eq!(devices[0]["waiting"], true);
    assert_eq!(devices[0]["requested"], true);

    let (status, _, _) = call(&app, browser("POST", &format!("/admin/devices/{ID}/forget"), &cookies, "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list(app.clone(), cookies.clone()).await, serde_json::json!([]));
    let (status, _, _) = call(&app, browser("POST", &format!("/admin/devices/{ID}/waiting"), &cookies, "on=1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
