use std::time::Duration;

use cln_plugin::Plugin;

use crate::{mint::LABEL_PREFIX, structs::PluginState};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// Credits a note as soon as its mint invoice is paid. Walks every invoice
/// paid since the last `pay_index` handled, so a payment that landed while
/// the plugin was down is credited on startup.
pub async fn invoice_watcher(plugin: Plugin<PluginState>) -> Result<(), anyhow::Error> {
    let state = plugin.state();
    let mut lastpay_index = state.store.pay_index()?;
    loop {
        match state.node.wait_any_invoice(lastpay_index).await {
            Ok((label, payment_hash, pay_index)) => {
                if label.starts_with(LABEL_PREFIX) {
                    if let Err(e) = state.settle_mint(&payment_hash) {
                        log::warn!("could not credit mint {payment_hash}: {}", e.reason());
                        tokio::time::sleep(RETRY_DELAY).await;
                        continue;
                    }
                }
                lastpay_index = pay_index;
                state.store.set_pay_index(lastpay_index)?;
            }
            Err(e) => {
                log::warn!("waitanyinvoice failed: {e:#}");
                tokio::time::sleep(RETRY_DELAY).await;
            }
        }
    }
}

/// Resolves melts a crash, a restart or an unknown payment outcome left
/// pending: at startup, then every minute.
pub async fn melt_reconciler(plugin: Plugin<PluginState>) -> Result<(), anyhow::Error> {
    loop {
        if let Err(e) = plugin.state().reconcile_pending_melts().await {
            log::warn!("reconciling pending melts failed: {}", e.reason());
        }
        tokio::time::sleep(RECONCILE_INTERVAL).await;
    }
}
