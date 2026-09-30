//! cln-mint: an LNURLcash mint (LUD-25 bearer notes, LUD-26 derivation and
//! Lightning Address auto-mint) as a Core Lightning plugin.

use cln_plugin::{
    RpcMethodBuilder,
    options::{
        ConfigOption, DefaultBooleanConfigOption, DefaultIntegerConfigOption,
        DefaultStringConfigOption, StringConfigOption,
    },
};
use parse::get_startup_options;
use tokio::io::{stdin, stdout};

mod db;
mod kernel;
mod lnurl;
mod mint;
mod node;
mod parse;
mod rpc;
mod spend;
mod structs;
mod tasks;
#[cfg(test)]
mod vectors;

const OPT_BASE_URL: StringConfigOption = ConfigOption::new_str_no_default(
    "cln-mint-base-url",
    "Public base URL of the mint, e.g. https://mint.example",
);
const OPT_ONION_URL: StringConfigOption = ConfigOption::new_str_no_default(
    "cln-mint-onion-url",
    "Tor hidden service base URL, used for requests arriving on that host",
);
const OPT_LISTEN: DefaultStringConfigOption = ConfigOption::new_str_with_default(
    "cln-mint-listen",
    "localhost:8899",
    "Listen address for the LNURL web server",
);
const OPT_DATABASE: StringConfigOption = ConfigOption::new_str_no_default(
    "cln-mint-database",
    "Path of the note database (default: <lightning-dir>/cln-mint.sqlite3)",
);
const OPT_BITCOINKERNEL: StringConfigOption = ConfigOption::new_str_no_default(
    "cln-mint-bitcoinkernel",
    "Path to libbitcoinkernel, to accept every tapscript leaf Bitcoin Core accepts",
);
const OPT_USERNAME: DefaultStringConfigOption = ConfigOption::new_str_with_default(
    "cln-mint-username",
    "mint",
    "The mint's own Lightning Address username (`_` always works too)",
);
const OPT_MIN_SENDABLE: DefaultIntegerConfigOption = ConfigOption::new_i64_with_default(
    "cln-mint-min-sendable-msat",
    10_000,
    "Smallest mint payment accepted, in msat",
);
const OPT_MAX_SENDABLE: DefaultIntegerConfigOption = ConfigOption::new_i64_with_default(
    "cln-mint-max-sendable-msat",
    1_000_000_000,
    "Largest mint payment accepted, in msat",
);
const OPT_BASE_FEE_MSAT: DefaultIntegerConfigOption = ConfigOption::new_i64_with_default(
    "cln-mint-base-fee-msat",
    1000,
    "Flat mint fee in msat, also charged on every split",
);
const OPT_FEE_PERCENT_PPM: DefaultIntegerConfigOption = ConfigOption::new_i64_with_default(
    "cln-mint-fee-percent-ppm",
    0,
    "Proportional mint fee in parts per million (max 100000)",
);
const OPT_MIN_MINT: DefaultIntegerConfigOption = ConfigOption::new_i64_with_default(
    "cln-mint-min-mint-msat",
    10_000,
    "Smallest value a freshly minted note may have, net of fees",
);
const OPT_MAX_K1S: DefaultIntegerConfigOption = ConfigOption::new_i64_with_default(
    "cln-mint-max-k1s",
    100,
    "Most notes a single callback may name",
);
const OPT_SUNSET_MINT: DefaultBooleanConfigOption = ConfigOption::new_bool_with_default(
    "cln-mint-sunset-mint",
    false,
    "Wind the mint down: refuse mints and splits, keep rotate, merge and melt",
);
const OPT_SUNSET_DATE: StringConfigOption = ConfigOption::new_str_no_default(
    "cln-mint-sunset-date",
    "Planned shutdown date to advertise (ISO-8601)",
);
const OPT_VERIFY: DefaultBooleanConfigOption = ConfigOption::new_bool_with_default(
    "cln-mint-verify",
    true,
    "Serve LUD-21 /verify for mint invoices and melts",
);
const OPT_REGISTRATION: DefaultBooleanConfigOption = ConfigOption::new_bool_with_default(
    "cln-mint-username-registration",
    true,
    "Let wallets register a Lightning Address against a cx1 branch (LUD-26)",
);
const OPT_NIP05: DefaultBooleanConfigOption = ConfigOption::new_bool_with_default(
    "cln-mint-nip05",
    true,
    "Serve /.well-known/nostr.json for registered usernames",
);
const OPT_TITLE: DefaultStringConfigOption =
    ConfigOption::new_str_with_default("cln-mint-title", "cln-mint", "Title of the web page");
const OPT_DESCRIPTION: DefaultStringConfigOption = ConfigOption::new_str_with_default(
    "cln-mint-description",
    "A minimal LNURLcash mint.",
    "Description on the web page",
);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    unsafe {
        std::env::set_var(
            "CLN_PLUGIN_LOG",
            "cln_plugin=info,cln_rpc=info,cln_mint=debug,info",
        );
    };
    let Some(configured_plugin) = cln_plugin::Builder::new(stdin(), stdout())
        .option(OPT_BASE_URL)
        .option(OPT_ONION_URL)
        .option(OPT_LISTEN)
        .option(OPT_DATABASE)
        .option(OPT_BITCOINKERNEL)
        .option(OPT_USERNAME)
        .option(OPT_MIN_SENDABLE)
        .option(OPT_MAX_SENDABLE)
        .option(OPT_BASE_FEE_MSAT)
        .option(OPT_FEE_PERCENT_PPM)
        .option(OPT_MIN_MINT)
        .option(OPT_MAX_K1S)
        .option(OPT_SUNSET_MINT)
        .option(OPT_SUNSET_DATE)
        .option(OPT_VERIFY)
        .option(OPT_REGISTRATION)
        .option(OPT_NIP05)
        .option(OPT_TITLE)
        .option(OPT_DESCRIPTION)
        .rpcmethod_from_builder(
            RpcMethodBuilder::new("cln-mint-info", rpc::mint_info)
                .description("Show the mint's URLs, fees and totals"),
        )
        .rpcmethod_from_builder(
            RpcMethodBuilder::new("cln-mint-note", rpc::mint_note)
                .description("Look up a note by its cp1 or bearer hash")
                .usage("note"),
        )
        .rpcmethod_from_builder(
            RpcMethodBuilder::new("cln-mint-pending", rpc::mint_pending)
                .description("List notes reserved by in-flight melts"),
        )
        .rpcmethod_from_builder(
            RpcMethodBuilder::new("cln-mint-reconcile", rpc::mint_reconcile)
                .description("Resolve melts left pending, from listpays"),
        )
        .rpcmethod_from_builder(
            RpcMethodBuilder::new("cln-mint-listusers", rpc::mint_listusers)
                .description("List Lightning Address usernames registered against a cx1"),
        )
        .configure()
        .await?
    else {
        return Ok(());
    };

    let state = match get_startup_options(&configured_plugin) {
        Ok(s) => s,
        Err(e) => {
            return configured_plugin
                .disable(&format!("Error parsing options: {e}"))
                .await;
        }
    };

    let listener = match tokio::net::TcpListener::bind(&state.listen_address).await {
        Ok(o) => o,
        Err(e) => {
            return configured_plugin
                .disable(&format!("Error binding to listen address: {e}"))
                .await;
        }
    };
    let router = lnurl::router(state.clone());

    let plugin = configured_plugin.start(state.clone()).await?;

    let (base, host) = state.settings.public_base_url_and_host(None);
    log::info!(
        "Starting mint server. LISTEN:{} BASE_URL:{base} ADDRESS:{}@{host}",
        state.listen_address,
        state.settings.username
    );

    let plugin_clone = plugin.clone();
    tokio::spawn(async move {
        match axum::serve(listener, router.into_make_service()).await {
            Ok(()) => _ = plugin_clone.shutdown(),
            Err(e) => {
                log_error(&format!("Error running server: {e}"));
                _ = plugin_clone.shutdown();
            }
        }
    });
    for (name, task) in [
        (
            "invoice_watcher",
            tokio::spawn(tasks::invoice_watcher(plugin.clone())),
        ),
        (
            "melt_reconciler",
            tokio::spawn(tasks::melt_reconciler(plugin.clone())),
        ),
    ] {
        let plugin_clone = plugin.clone();
        tokio::spawn(async move {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log_error(&format!("Error running {name}: {e}")),
                Err(e) => log_error(&format!("{name} panicked: {e}")),
            }
            _ = plugin_clone.shutdown();
        });
    }

    plugin.join().await
}

fn log_error(error: &str) {
    println!(
        "{}",
        serde_json::json!({"jsonrpc": "2.0",
                          "method": "log",
                          "params": {"level":"warn", "message":error}})
    );
}
