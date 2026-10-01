use std::{
    net::{SocketAddr, ToSocketAddrs},
    path::Path,
};

use anyhow::anyhow;
use cln_plugin::ConfiguredPlugin;

use crate::{
    OPT_BASE_FEE_MSAT, OPT_BASE_URL, OPT_DATABASE, OPT_DESCRIPTION, OPT_FEE_PERCENT_PPM,
    OPT_LISTEN, OPT_MAX_K1S, OPT_MAX_SENDABLE, OPT_MIN_MINT, OPT_MIN_SENDABLE, OPT_NIP05,
    OPT_ONION_URL, OPT_REGISTRATION, OPT_SUNSET_DATE, OPT_SUNSET_MINT, OPT_TITLE, OPT_USERNAME,
    OPT_VERIFY,
    db::NoteStore,
    node::Node,
    structs::{PluginState, Settings},
};

const DEFAULT_DATABASE_FILENAME: &str = "cln-mint.sqlite3";

fn non_negative(name: &str, value: i64) -> Result<u64, anyhow::Error> {
    u64::try_from(value).map_err(|_| anyhow!("`{name}` must not be negative"))
}

pub fn get_startup_options(
    plugin: &ConfiguredPlugin<PluginState, tokio::io::Stdin, tokio::io::Stdout>,
) -> Result<PluginState, anyhow::Error> {
    let config = plugin.configuration();
    let lightning_dir = Path::new(&config.lightning_dir);
    let rpc_path = lightning_dir.join(&config.rpc_file);

    let listen_opt = plugin.option(&OPT_LISTEN)?;
    let listen_address: SocketAddr = listen_opt
        .to_socket_addrs()
        .map_err(|e| anyhow!("`{}` is invalid: {e}", OPT_LISTEN.name()))?
        .next()
        .ok_or_else(|| anyhow!("`{}` resolves to no address", OPT_LISTEN.name()))?;

    let base_url = plugin.option(&OPT_BASE_URL)?.ok_or_else(|| {
        anyhow!(
            "Please specify `{}`, e.g. https://mint.example",
            OPT_BASE_URL.name()
        )
    })?;

    let database_path = match plugin.option(&OPT_DATABASE)? {
        Some(path) => path,
        None => lightning_dir
            .join(DEFAULT_DATABASE_FILENAME)
            .to_string_lossy()
            .into_owned(),
    };

    let settings = Settings {
        base_url,
        onion_url: plugin.option(&OPT_ONION_URL)?,
        username: plugin.option(&OPT_USERNAME)?.to_ascii_lowercase(),
        min_sendable_msat: non_negative(
            OPT_MIN_SENDABLE.name(),
            plugin.option(&OPT_MIN_SENDABLE)?,
        )?,
        max_sendable_msat: non_negative(
            OPT_MAX_SENDABLE.name(),
            plugin.option(&OPT_MAX_SENDABLE)?,
        )?,
        base_fee_msat: non_negative(OPT_BASE_FEE_MSAT.name(), plugin.option(&OPT_BASE_FEE_MSAT)?)?,
        fee_percent_ppm: non_negative(
            OPT_FEE_PERCENT_PPM.name(),
            plugin.option(&OPT_FEE_PERCENT_PPM)?,
        )?,
        min_mint_msat: non_negative(OPT_MIN_MINT.name(), plugin.option(&OPT_MIN_MINT)?)?,
        max_k1s: non_negative(OPT_MAX_K1S.name(), plugin.option(&OPT_MAX_K1S)?)? as usize,
        sunset_mint: plugin.option(&OPT_SUNSET_MINT)?,
        sunset_date: plugin.option(&OPT_SUNSET_DATE)?,
        verify_enabled: plugin.option(&OPT_VERIFY)?,
        username_registration_enabled: plugin.option(&OPT_REGISTRATION)?,
        nip05_enabled: plugin.option(&OPT_NIP05)?,
        title: plugin.option(&OPT_TITLE)?,
        description: plugin.option(&OPT_DESCRIPTION)?,
    };
    settings.validate()?;
    if !crate::mint::valid_username(&settings.username) && settings.username != "_" {
        return Err(anyhow!("`{}` is not a valid username", OPT_USERNAME.name()));
    }

    let store = NoteStore::open(&database_path)
        .map_err(|e| anyhow!("Could not open database {database_path}: {e}"))?;
    log::info!("Using database {database_path}");

    Ok(PluginState::new(
        settings,
        store,
        Node::new(rpc_path),
        listen_address,
    ))
}
