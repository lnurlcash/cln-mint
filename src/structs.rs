//! Plugin state: the options read at startup, the store and the node.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use anyhow::{Result, bail};
use tokio::sync::OnceCell;

use crate::{db::NoteStore, kernel::Kernel, node::Node};

/// Everything the HTTP handlers, RPC methods and background tasks share.
#[derive(Debug, Clone)]
pub struct PluginState {
    pub settings: Arc<Settings>,
    pub store: Arc<NoteStore>,
    pub node: Node,
    /// Bitcoin Core's interpreter, for leaves `lnurlcash-core` cannot judge.
    pub kernel: Option<Arc<Kernel>>,
    pub listen_address: SocketAddr,
    /// This node's id: the `mintPubkey` certificates are signed with.
    pub node_id: Arc<OnceCell<String>>,
    /// Payment hashes with a melt attempt live in this process, refcounted.
    /// Reconciliation leaves these alone: `listpays` can report a payment
    /// unknown simply because `xpay` has not reached it yet.
    pub in_flight_melts: Arc<Mutex<HashMap<String, usize>>>,
}

impl PluginState {
    pub fn new(
        settings: Settings,
        store: NoteStore,
        node: Node,
        kernel: Option<Kernel>,
        listen_address: SocketAddr,
    ) -> Self {
        PluginState {
            settings: Arc::new(settings),
            store: Arc::new(store),
            node,
            kernel: kernel.map(Arc::new),
            listen_address,
            node_id: Arc::new(OnceCell::new()),
            in_flight_melts: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Everything configurable, with lnurl-mint's defaults.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Public base URL of the mint (scheme and host, optional port and path).
    pub base_url: String,
    /// A Tor hidden service base URL, used instead of `base_url` for requests
    /// arriving on its host.
    pub onion_url: Option<String>,
    /// The mint's own Lightning Address local part (`_` always works too).
    pub username: String,
    pub min_sendable_msat: u64,
    pub max_sendable_msat: u64,
    pub base_fee_msat: u64,
    pub fee_percent_ppm: u64,
    /// Floor on a freshly minted note's value, net of the mint fee.
    pub min_mint_msat: u64,
    pub max_k1s: usize,
    /// Refuse mints and splits: holders can still rotate, merge and melt.
    pub sunset_mint: bool,
    pub sunset_date: Option<String>,
    pub verify_enabled: bool,
    pub username_registration_enabled: bool,
    pub nip05_enabled: bool,
    pub title: String,
    pub description: String,
}

pub fn hostname(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .host_str()
        .map(|h| h.trim_matches(['[', ']']).to_ascii_lowercase())
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        if hostname(&self.base_url).is_none() {
            bail!("cln-mint-base-url {:?} has no hostname", self.base_url);
        }
        if let Some(onion) = &self.onion_url {
            if hostname(onion).is_none() {
                bail!("cln-mint-onion-url {onion:?} has no hostname");
            }
        }
        if self.min_sendable_msat == 0 || self.min_sendable_msat > self.max_sendable_msat {
            bail!("cln-mint-min-sendable-msat must be in [1, cln-mint-max-sendable-msat]");
        }
        if self.fee_percent_ppm > 100_000 {
            bail!("cln-mint-fee-percent-ppm is capped at 100000 (10%)");
        }
        if self.max_k1s == 0 {
            bail!("cln-mint-max-k1s must be at least 1");
        }
        Ok(())
    }

    /// The base URL to hand out for a request that arrived on `request_host`:
    /// the onion URL for a Tor visitor, `base_url` otherwise. Never the Host
    /// header itself, which anyone can spoof.
    pub fn public_base_url(&self, request_host: Option<&str>) -> String {
        if let (Some(onion), Some(host)) = (&self.onion_url, request_host) {
            let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
            if hostname(onion).as_deref() == Some(host.as_str()) {
                return onion.trim_end_matches('/').to_string();
            }
        }
        self.base_url.trim_end_matches('/').to_string()
    }

    pub fn public_base_url_and_host(&self, request_host: Option<&str>) -> (String, String) {
        let base = self.public_base_url(request_host);
        let host = hostname(&base).unwrap_or_default();
        (base, host)
    }

    /// Every host this mint answers on: the domains a spend may be bound to.
    pub fn spend_domains(&self) -> Vec<String> {
        let mut hosts = vec![];
        for url in std::iter::once(&self.base_url).chain(self.onion_url.as_ref()) {
            if let Some(h) = hostname(url) {
                if !hosts.contains(&h) {
                    hosts.push(h);
                }
            }
        }
        hosts
    }

    /// The fee withheld from a mint of `amount_msat`, rounded up to a sat.
    pub fn mint_fee_msat(&self, amount_msat: u64) -> u64 {
        let percent = (amount_msat / 1_000_000) * self.fee_percent_ppm
            + ((amount_msat % 1_000_000) * self.fee_percent_ppm) / 1_000_000;
        (self.base_fee_msat + percent).div_ceil(1000) * 1000
    }

    /// The smallest amount worth advertising: one whose net clears `min_mint_msat`.
    pub fn min_sendable(&self) -> u64 {
        let mut amount = self.min_sendable_msat.max(self.min_mint_msat);
        // terminates: the fee is capped at 10% plus a constant
        while amount.saturating_sub(self.mint_fee_msat(amount)) < self.min_mint_msat {
            amount += 1000;
        }
        amount
    }

    /// The largest value a freshly minted note can have.
    pub fn max_mintable(&self) -> u64 {
        self.max_sendable_msat
            .saturating_sub(self.mint_fee_msat(self.max_sendable_msat))
    }

    /// Routing budget for melting a note worth `amount_msat`: the mint fee it
    /// paid, never under 0.5% or 5 sat.
    pub fn melt_fee_limit_msat(&self, amount_msat: u64) -> u64 {
        (amount_msat / 200)
            .max(5000)
            .max(self.mint_fee_msat(amount_msat))
    }

    pub fn has_fee(&self) -> bool {
        self.base_fee_msat != 0 || self.fee_percent_ppm != 0
    }
}

#[cfg(test)]
pub fn test_settings() -> Settings {
    Settings {
        base_url: "https://mint.example".into(),
        onion_url: Some("http://abc.onion".into()),
        username: "mint".into(),
        min_sendable_msat: 10_000,
        max_sendable_msat: 1_000_000_000,
        base_fee_msat: 1000,
        fee_percent_ppm: 2000,
        min_mint_msat: 10_000,
        max_k1s: 100,
        sunset_mint: false,
        sunset_date: None,
        verify_enabled: true,
        username_registration_enabled: true,
        nip05_enabled: true,
        title: "cln-mint".into(),
        description: "".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fees_round_up_to_a_sat() {
        let s = test_settings();
        // 1000 + 0.2% of 1_000_000 = 3000
        assert_eq!(s.mint_fee_msat(1_000_000), 3000);
        // 1000 + 0.2% of 10_500 = 1021 -> 2000
        assert_eq!(s.mint_fee_msat(10_500), 2000);
        assert!(s.min_sendable() - s.mint_fee_msat(s.min_sendable()) >= s.min_mint_msat);
    }

    #[test]
    fn domains_and_onion_base() {
        let s = test_settings();
        assert_eq!(s.spend_domains(), vec!["mint.example", "abc.onion"]);
        assert_eq!(s.public_base_url(Some("abc.onion")), "http://abc.onion");
        assert_eq!(
            s.public_base_url(Some("evil.example")),
            "https://mint.example"
        );
    }
}
