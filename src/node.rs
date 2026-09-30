//! This node, over its own JSON-RPC socket.
//!
//! Every call opens its own connection: `ClnRpc` needs `&mut self`, a unix
//! socket is cheap, and a melt's `xpay` can block for a minute without
//! holding up every other request.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use cln_rpc::{ClnRpc, RpcError};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// xpay's documented failure codes.
const XPAY_FAILURES: [(i32, &str); 4] = [
    (
        203,
        "The invoice's destination permanently rejected this payment.",
    ),
    (205, "Could not find a route to pay this invoice."),
    (207, "This invoice has expired."),
    (219, "This invoice has already been paid."),
];

#[derive(Debug, Clone)]
pub struct Node {
    rpc_path: PathBuf,
}

/// A created invoice and its payment hash.
#[derive(Debug, Clone)]
pub struct Invoice {
    pub bolt11: String,
    pub payment_hash: String,
}

/// What `decode` says about a BOLT-11 invoice a WALLET handed in.
#[derive(Debug, Clone)]
pub struct DecodedInvoice {
    pub amount_msat: Option<u64>,
    pub payment_hash: String,
}

/// Where an outgoing payment stands, as `listpays` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayStatus {
    Complete,
    /// Nothing complete or pending: no HTLC is outstanding.
    Failed,
    /// Not terminal: an HTLC may still be held.
    Pending,
}

#[derive(Debug, Clone)]
pub struct PayResult {
    pub fee_msat: Option<u64>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct NodeInfo {
    pub id: String,
    pub alias: Option<String>,
    pub color: Option<String>,
    pub uris: Vec<String>,
    pub num_peers: Option<u64>,
    pub num_channels: Option<u64>,
    pub capacity_msat: Option<u64>,
}

#[derive(Debug)]
pub enum PayError {
    /// The node answered with a failure. Not proof that no HTLC is left.
    Failed(String),
    /// The call itself broke: the outcome is unknown.
    Unknown(anyhow::Error),
}

fn msat(value: &Value) -> Option<u64> {
    // cln returns msat as integers; older versions as "123msat"
    value.as_u64().or_else(|| {
        value
            .as_str()
            .and_then(|s| s.trim_end_matches("msat").parse().ok())
    })
}

impl Node {
    pub fn new(rpc_path: impl Into<PathBuf>) -> Self {
        Node {
            rpc_path: rpc_path.into(),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let mut rpc = ClnRpc::new(&self.rpc_path).await.map_err(|e| RpcError {
            code: None,
            message: format!("could not connect to lightningd: {e}"),
            data: None,
        })?;
        rpc.call_raw(method, &params).await
    }

    async fn call_ok(&self, method: &str, params: Value) -> Result<Value> {
        self.call(method, params)
            .await
            .map_err(|e| anyhow!("{method}: {}", e.message))
    }

    /// An invoice for `amount_msat` committing to `description` by hash only
    /// (LUD-06's `description_hash`), labelled so the invoice watcher (`tasks.rs`) finds it.
    pub async fn create_invoice(
        &self,
        amount_msat: u64,
        description: &str,
        label_prefix: &str,
    ) -> Result<Invoice> {
        let preimage: [u8; 32] = rand::random();
        let payment_hash = hex::encode(Sha256::digest(preimage));
        let res = self
            .call_ok(
                "invoice",
                json!({
                    "amount_msat": amount_msat,
                    "label": format!("{label_prefix}{payment_hash}"),
                    "description": description,
                    "deschashonly": true,
                    "preimage": hex::encode(preimage),
                }),
            )
            .await?;
        let bolt11 = res["bolt11"]
            .as_str()
            .context("invoice returned no bolt11")?
            .to_string();
        if res["payment_hash"].as_str() != Some(payment_hash.as_str()) {
            bail!("invoice returned an unexpected payment_hash");
        }
        Ok(Invoice {
            bolt11,
            payment_hash,
        })
    }

    /// Block until an invoice is paid after `lastpay_index`: its label,
    /// payment hash and `pay_index`.
    pub async fn wait_any_invoice(&self, lastpay_index: u64) -> Result<(String, String, u64)> {
        let res = self
            .call_ok("waitanyinvoice", json!({"lastpay_index": lastpay_index}))
            .await?;
        Ok((
            res["label"].as_str().unwrap_or_default().to_string(),
            res["payment_hash"].as_str().unwrap_or_default().to_string(),
            res["pay_index"]
                .as_u64()
                .context("waitanyinvoice returned no pay_index")?,
        ))
    }

    /// Whether an invoice this node issued is paid, and its preimage if so.
    pub async fn invoice_paid(&self, payment_hash: &str) -> Result<Option<Option<String>>> {
        let res = self
            .call_ok("listinvoices", json!({"payment_hash": payment_hash}))
            .await?;
        let paid = res["invoices"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|inv| inv["status"] == "paid");
        Ok(paid.map(|inv| inv["payment_preimage"].as_str().map(str::to_string)))
    }

    pub async fn decode_invoice(&self, bolt11: &str) -> Result<DecodedInvoice> {
        let res = self.call_ok("decode", json!({"string": bolt11})).await?;
        if res["type"] != "bolt11 invoice" {
            bail!("not a BOLT-11 invoice");
        }
        if res["valid"] != true {
            bail!("invalid invoice");
        }
        Ok(DecodedInvoice {
            amount_msat: msat(&res["amount_msat"]),
            payment_hash: res["payment_hash"]
                .as_str()
                .context("invoice has no payment hash")?
                .to_string(),
        })
    }

    /// Pay `bolt11` with xpay, spending at most `maxfee_msat` on routing.
    pub async fn pay(&self, bolt11: &str, maxfee_msat: u64) -> Result<PayResult, PayError> {
        let res = self
            .call("xpay", json!({"invstring": bolt11, "maxfee": maxfee_msat}))
            .await;
        match res {
            Ok(payment) => {
                if payment["payment_preimage"].as_str().is_none() {
                    return Err(PayError::Unknown(anyhow!("xpay returned no preimage")));
                }
                let fee_msat = match (
                    msat(&payment["amount_sent_msat"]),
                    msat(&payment["amount_msat"]),
                ) {
                    (Some(sent), Some(received)) => sent.checked_sub(received),
                    _ => None,
                };
                Ok(PayResult { fee_msat })
            }
            // no code: the socket or the transport broke, not the payment
            Err(err) if err.code.is_none() => Err(PayError::Unknown(anyhow!(err.message))),
            Err(err) => {
                let reason = XPAY_FAILURES
                    .iter()
                    .find(|(code, _)| Some(*code) == err.code)
                    .map(|(_, r)| r.to_string())
                    .unwrap_or(err.message);
                Err(PayError::Failed(reason))
            }
        }
    }

    /// Where the outgoing payment to `payment_hash` stands.
    pub async fn pay_status(&self, payment_hash: &str) -> Result<(PayStatus, Option<String>)> {
        let res = self
            .call_ok("listpays", json!({"payment_hash": payment_hash}))
            .await?;
        let pays: Vec<&Value> = res["pays"].as_array().into_iter().flatten().collect();
        if let Some(done) = pays.iter().find(|p| p["status"] == "complete") {
            return Ok((
                PayStatus::Complete,
                done["preimage"].as_str().map(str::to_string),
            ));
        }
        if pays.iter().any(|p| p["status"] == "pending") {
            return Ok((PayStatus::Pending, None));
        }
        Ok((PayStatus::Failed, None))
    }

    /// `signmessage`: r || s || recovery id, 65 bytes, as LUD-25's `cs1` wants.
    pub async fn sign_message(&self, message: &str) -> Result<[u8; 65]> {
        let res = self
            .call_ok("signmessage", json!({"message": message}))
            .await?;
        let signature = hex::decode(res["signature"].as_str().context("no signature")?)?;
        let recid = hex::decode(res["recid"].as_str().context("no recid")?)?;
        if signature.len() != 64 || recid.len() != 1 {
            bail!("signmessage returned a malformed signature");
        }
        let mut out = [0u8; 65];
        out[..64].copy_from_slice(&signature);
        out[64] = recid[0];
        Ok(out)
    }

    pub async fn info(&self) -> Result<NodeInfo> {
        let res = self.call_ok("getinfo", json!({})).await?;
        let id = res["id"]
            .as_str()
            .context("getinfo returned no id")?
            .to_string();
        let uris = res["address"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let port = a["port"].as_u64()?;
                let addr = a["address"].as_str()?;
                Some(if a["type"] == "ipv6" {
                    format!("{id}@[{addr}]:{port}")
                } else {
                    format!("{id}@{addr}:{port}")
                })
            })
            .collect();
        let capacity_msat = match self.call("listpeerchannels", json!({})).await {
            Ok(channels) => Some(
                channels["channels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| c["state"] == "CHANNELD_NORMAL")
                    .filter_map(|c| msat(&c["total_msat"]))
                    .sum(),
            ),
            Err(_) => None,
        };
        Ok(NodeInfo {
            alias: res["alias"].as_str().map(str::to_string),
            color: res["color"].as_str().map(|c| format!("#{c}")),
            uris,
            num_peers: res["num_peers"].as_u64(),
            num_channels: res["num_active_channels"].as_u64(),
            capacity_msat,
            id,
        })
    }
}
