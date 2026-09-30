use anyhow::anyhow;
use cln_plugin::Plugin;
use lnurlcash_core::recoverable::encode_cx1;
use serde_json::{Value, json};

use crate::{spend, structs::PluginState};

/// A positional or named string parameter.
fn string_param(args: &Value, index: usize, name: &str) -> Option<String> {
    match args {
        Value::Array(a) => a.get(index).and_then(Value::as_str).map(str::to_string),
        Value::Object(o) => o.get(name).and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

pub async fn mint_info(plugin: Plugin<PluginState>, _args: Value) -> Result<Value, anyhow::Error> {
    let state = plugin.state();
    let s = &state.settings;
    let (base, host) = s.public_base_url_and_host(None);
    Ok(json!({
        "base_url": base,
        "onion_url": s.onion_url,
        "listen": state.listen_address.to_string(),
        "lightning_address": format!("{}@{host}", s.username),
        "lnurl": lnurlcash_core::to_bech32_lnurl(&format!("{base}/.well-known/lnurlp/{}", s.username))
            .map(|l| l.to_ascii_uppercase()),
        "withdraw_link": format!("{base}/w"),
        "spend_domains": s.spend_domains(),
        "mint_pubkey": state.mint_pubkey().await,
        "min_sendable_msat": s.min_sendable(),
        "max_sendable_msat": s.max_sendable_msat,
        "base_fee_msat": s.base_fee_msat,
        "fee_percent_ppm": s.fee_percent_ppm,
        "sunset_mint": s.sunset_mint,
        "stats": state.store.stats()?,
    }))
}

pub async fn mint_note(plugin: Plugin<PluginState>, args: Value) -> Result<Value, anyhow::Error> {
    let note =
        string_param(&args, 0, "note").ok_or_else(|| anyhow!("usage: cln-mint-note note"))?;
    let note_id =
        spend::note_id_of_ref(&note).ok_or_else(|| anyhow!("not a cp1 or a bearer note's hash"))?;
    let state = plugin.state();
    let Some(record) = state.store.note_record(&note_id)? else {
        let unpaid_mint = state.store.pending_mint_by_note_id(&note_id)?;
        return Ok(json!({"note_id": note_id, "status": "unknown", "unpaid_mint": unpaid_mint}));
    };
    let status = match (record.spent, record.pending) {
        (true, _) => "spent",
        (false, true) => "pending",
        (false, false) => "outstanding",
    };
    Ok(json!({
        "note_id": note_id,
        "status": status,
        "amount_msat": record.amount_msat,
        "locked_at": record.locked_at,
    }))
}

pub async fn mint_pending(
    plugin: Plugin<PluginState>,
    _args: Value,
) -> Result<Value, anyhow::Error> {
    let melts = plugin.state().store.pending_melts()?;
    Ok(json!({"pending_melts": melts}))
}

pub async fn mint_reconcile(
    plugin: Plugin<PluginState>,
    _args: Value,
) -> Result<Value, anyhow::Error> {
    plugin
        .state()
        .reconcile_pending_melts()
        .await
        .map_err(|e| anyhow!(e.reason()))
}

pub async fn mint_listusers(
    plugin: Plugin<PluginState>,
    _args: Value,
) -> Result<Value, anyhow::Error> {
    let users: Vec<Value> = plugin
        .state()
        .store
        .list_usernames()?
        .into_iter()
        .map(|(username, branch_hex, next_index)| {
            let cx1 = hex::decode(&branch_hex)
                .ok()
                .filter(|b| b.len() == 64)
                .map(|b| encode_cx1(b[..32].try_into().unwrap(), b[32..].try_into().unwrap()));
            json!({"username": username, "cx1": cx1, "next_index": next_index})
        })
        .collect();
    Ok(json!({"users": users}))
}
