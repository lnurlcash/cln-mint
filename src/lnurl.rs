//! The HTTP side: LNURL endpoints, always answering 200 with LUD-01's
//! `{"status": "ERROR", "reason"}` on failure.

use axum::{
    Json, Router,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};

use crate::{
    mint::{MintError, MintResult},
    structs::PluginState,
};

pub fn router(state: PluginState) -> Router {
    Router::new()
        .route("/", get(frontend))
        .route("/.well-known/lnurlp/{username}", get(lnurlp))
        .route("/.well-known/lnurlw/{username}", get(lnurlw))
        .route("/.well-known/nostr.json", get(nostr_json))
        .route("/p/cb", get(pay_callback))
        .route(
            "/p/{username}",
            get(pay_callback_for_username)
                .post(register_username)
                .delete(unregister_username),
        )
        .route("/verify/{payment_hash}", get(verify))
        .route("/w", get(withdraw_request))
        .route("/w/cb", get(withdraw_callback))
        .with_state(state)
}

fn respond(result: MintResult<Value>) -> Response {
    let body = result.unwrap_or_else(|e| json!({"status": "ERROR", "reason": e.reason()}));
    ([(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")], Json(body)).into_response()
}

fn request_host(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::HOST).and_then(|h| h.to_str().ok())
}

/// The query as ordered pairs: `k1` repeats for a merge.
fn query_pairs(query: Option<String>) -> Vec<(String, String)> {
    url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .into_owned()
        .collect()
}

fn param<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn amount_param(pairs: &[(String, String)], name: &str) -> MintResult<Option<u64>> {
    match param(pairs, name) {
        None => Ok(None),
        Some(v) => v
            .parse()
            .map(Some)
            .map_err(|_| MintError::Reject(format!("Invalid {name}."))),
    }
}

fn required<'a>(pairs: &'a [(String, String)], name: &str) -> MintResult<&'a str> {
    param(pairs, name).ok_or_else(|| MintError::Reject(format!("Missing {name}.")))
}

async fn lnurlp(
    State(state): State<PluginState>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    respond(state.pay_request(&username, request_host(&headers)))
}

async fn lnurlw(
    State(state): State<PluginState>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    respond(state.mint_address(&username, request_host(&headers)).await)
}

async fn nostr_json(State(state): State<PluginState>, RawQuery(query): RawQuery) -> Response {
    let pairs = query_pairs(query);
    respond(state.nip05(param(&pairs, "name")))
}

async fn pay_callback(
    State(state): State<PluginState>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let pairs = query_pairs(query);
    respond(pay(&state, None, &pairs, request_host(&headers)).await)
}

async fn pay_callback_for_username(
    State(state): State<PluginState>,
    Path(username): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let pairs = query_pairs(query);
    respond(pay(&state, Some(&username), &pairs, request_host(&headers)).await)
}

async fn pay(
    state: &PluginState,
    username: Option<&str>,
    pairs: &[(String, String)],
    host: Option<&str>,
) -> MintResult<Value> {
    if param(pairs, "nostr").is_some() {
        return Err(MintError::Reject(
            "Zaps are not offered for this address.".into(),
        ));
    }
    let amount = amount_param(pairs, "amount")?
        .ok_or_else(|| MintError::Reject("Missing amount.".into()))?;
    state
        .pay_callback(username, amount, param(pairs, "comment"), host)
        .await
}

async fn register_username(
    State(state): State<PluginState>,
    Path(username): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let pairs = query_pairs(query);
    respond((|| {
        state.register(
            &username,
            required(&pairs, "cx1")?,
            required(&pairs, "sig")?,
            param(&pairs, "npub"),
            request_host(&headers),
        )
    })())
}

async fn unregister_username(
    State(state): State<PluginState>,
    Path(username): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let pairs = query_pairs(query);
    respond(
        required(&pairs, "sig")
            .and_then(|sig| state.unregister(&username, sig, request_host(&headers))),
    )
}

async fn verify(State(state): State<PluginState>, Path(payment_hash): Path<String>) -> Response {
    respond(state.verify(&payment_hash.to_ascii_lowercase()).await)
}

async fn withdraw_request(
    State(state): State<PluginState>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    // `amount` and `c` ride along on a note URL; they are ignored here
    let pairs = query_pairs(query);
    respond(
        state
            .withdraw_request(
                param(&pairs, "k1"),
                param(&pairs, "p"),
                request_host(&headers),
            )
            .await,
    )
}

async fn withdraw_callback(
    State(state): State<PluginState>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let pairs = query_pairs(query);
    let result = async {
        if param(&pairs, "p").is_some() {
            return Err(MintError::Reject(
                "p is only accepted at the informational endpoint.".into(),
            ));
        }
        let k1s: Vec<String> = pairs
            .iter()
            .filter(|(k, _)| k == "k1")
            .map(|(_, v)| v.clone())
            .collect();
        state
            .withdraw_callback(
                &k1s,
                param(&pairs, "pr"),
                amount_param(&pairs, "amount")?,
                param(&pairs, "p1"),
                param(&pairs, "p2"),
                request_host(&headers),
            )
            .await
    }
    .await;
    respond(result)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A one-page frontend: the mint's Lightning Address and LNURL as a QR code.
async fn frontend(State(state): State<PluginState>, headers: HeaderMap) -> Response {
    let s = &state.settings;
    let (base, host) = s.public_base_url_and_host(request_host(&headers));
    let address = format!("{}@{host}", s.username);
    let lnurl =
        lnurlcash_core::to_bech32_lnurl(&format!("{base}/.well-known/lnurlp/{}", s.username))
            .map(|l| l.to_ascii_uppercase())
            .unwrap_or_default();
    let qr = qrcode::QrCode::new(format!("lightning:{lnurl}").as_bytes())
        .map(|code| {
            code.render::<qrcode::render::svg::Color>()
                .min_dimensions(240, 240)
                .dark_color(qrcode::render::svg::Color("#111"))
                .light_color(qrcode::render::svg::Color("#fff"))
                .build()
        })
        .unwrap_or_default();
    let outstanding = state
        .store
        .stats()
        .map(|st| st.outstanding_msat / 1000)
        .unwrap_or(0);
    let fees = if s.has_fee() {
        format!(
            "{} msat + {} ppm per mint",
            s.base_fee_msat, s.fee_percent_ppm
        )
    } else {
        "none".into()
    };
    let sunset = if s.sunset_mint {
        "<p class=warn>This mint is winding down: minting and splitting are disabled.</p>"
    } else {
        ""
    };
    Html(format!(
        r#"<!doctype html><html lang=en><head><meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1"><title>{title}</title>
<style>
:root{{color-scheme:light dark;--fg:#111;--bg:#fafafa;--muted:#666}}
@media (prefers-color-scheme:dark){{:root{{--fg:#eee;--bg:#161616;--muted:#999}}}}
body{{font:16px/1.5 system-ui,sans-serif;color:var(--fg);background:var(--bg);max-width:36rem;margin:2rem auto;padding:0 16px}}
.qr svg{{width:240px;height:240px;border-radius:8px}} code{{word-break:break-all}} .muted{{color:var(--muted)}} .warn{{color:#c33}}
</style></head><body>
<h1>{title}</h1><p>{description}</p>{sunset}
<p>Pay <b>{address}</b> from a wallet that can attach a LUD-12 comment to mint an
<a href="https://github.com/lnurl/luds/blob/luds/25.md">LNURLcash</a> bearer note.</p>
<div class=qr><a href="lightning:{lnurl}">{qr}</a></div>
<p><code>{lnurl}</code></p>
<p class=muted>Mint fee: {fees} &middot; Outstanding notes: {outstanding} sat</p>
</body></html>"#,
        title = escape(&s.title),
        description = escape(&s.description),
    ))
    .into_response()
}
