uniffi::include_scaffolding!("floresta");

use std::path::PathBuf;
use std::str::FromStr;

use bitcoin::hashes::Hash;

/// Default data directory when the caller does not supply one.
///
/// The on-disk layout here is chosen to match the Android app's per-package
/// data directory on the installed device.
const DEFAULT_DATA_DIR: &str = "/data/data/com.github.jvsena42.mandacaru/files";

#[derive(Debug, Clone)]
/// The Bitcoin network to run on.
pub enum Network {
    /// Bitcoin mainnet.
    Bitcoin,
    /// Bitcoin signet.
    Signet,
    /// Bitcoin testnet.
    Testnet,
    /// Bitcoin regtest.
    Regtest,
    /// Bitcoin testnet4.
    Testnet4,
}

impl From<Network> for bitcoin::Network {
    fn from(network: Network) -> bitcoin::Network {
        match network {
            Network::Bitcoin => bitcoin::Network::Bitcoin,
            Network::Signet => bitcoin::Network::Signet,
            Network::Testnet => bitcoin::Network::Testnet,
            Network::Regtest => bitcoin::Network::Regtest,
            Network::Testnet4 => bitcoin::Network::Testnet4,
        }
    }
}

#[derive(Debug, Clone)]
/// Configures the assume-valid behavior for script validation.
pub enum AssumeValidArg {
    /// Validate all scripts from genesis.
    Disabled,

    /// Use Floresta's hard-coded block hash.
    Hardcoded,

    /// Use a user-provided block hash (64-character hex string).
    UserInput { block_hash: String },
}

#[derive(Debug, Clone)]
/// A pre-computed Utreexo accumulator state.
pub struct AssumeUtreexoValue {
    /// The block hash at which this accumulator state is valid.
    pub block_hash: String,

    /// The block height at which this accumulator state is valid.
    pub height: u32,

    /// The Utreexo accumulator roots at this block, as hex strings.
    pub roots: Vec<String>,

    /// The number of leaves in the Utreexo accumulator at this block.
    pub leaves: u64,
}

#[derive(Debug)]
/// Error returned by the Floresta FFI layer.
pub enum FlorestaFfiError {
    /// The daemon failed to start, with an error message.
    StartError { details: String },
}

impl std::fmt::Display for FlorestaFfiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StartError { details } => write!(f, "{details}"),
        }
    }
}

impl std::error::Error for FlorestaFfiError {}

/// A Floresta Bitcoin node instance.
///
/// Wraps the Floresta daemon and a Tokio runtime. Create with [`Florestad::new`]
/// for defaults or [`Florestad::from_config`] for custom settings. Call
/// [`Florestad::start`] to begin syncing and [`Florestad::stop`] before exit.
pub struct Florestad {
    rt: tokio::runtime::Runtime,
    florestad: floresta_node::Florestad,
    /// The network this daemon was constructed for. Stamped into
    /// `dump_utreexo_state` JSON payloads so the receiver can refuse cross-network
    /// imports.
    network: bitcoin::Network,
    _log_guard: Option<floresta_node::WorkerGuard>,
}

impl Florestad {
    /// Create a new Floresta node with default configuration.
    ///
    /// Uses Bitcoin mainnet and the Android per-package data directory.
    pub fn new() -> Florestad {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .thread_name("florestad")
            .build()
            .expect("failed to create tokio runtime");

        let data_dir = String::from(DEFAULT_DATA_DIR);
        let log_guard = floresta_node::init_logging(&data_dir, true, false, false)
            .ok()
            .flatten();
        let network = bitcoin::Network::Bitcoin;
        let florestad = floresta_node::Florestad::new(network, data_dir);
        Self {
            rt,
            florestad,
            network,
            _log_guard: log_guard,
        }
    }

    /// Create a new Floresta node with the given configuration.
    pub fn from_config(config: Config) -> Florestad {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .thread_name("florestad")
            .build()
            .expect("failed to create tokio runtime");

        let node_config: floresta_node::Config = config.into();
        let network = node_config.network;
        let log_guard = floresta_node::init_logging(
            &node_config.datadir.to_string_lossy(),
            node_config.log_to_file,
            node_config.log_to_stdout,
            node_config.debug,
        )
        .ok()
        .flatten();
        // Emit the snapshot summary here, once the subscriber is live, by
        // inspecting the already-converted node_config.
        match &node_config.assumeutreexo_value {
            Some(av) => tracing::info!(
                target: "floresta_ffi",
                "applying assumeutreexo_value: height={} leaves={} roots={} block_hash={}",
                av.height,
                av.leaves,
                av.roots.len(),
                av.block_hash,
            ),
            None => tracing::info!(
                target: "floresta_ffi",
                "no assumeutreexo_value; assume_utreexo={}",
                node_config.assume_utreexo,
            ),
        }
        let florestad = floresta_node::Florestad::from_config(node_config);
        Self {
            rt,
            florestad,
            network,
            _log_guard: log_guard,
        }
    }

    /// Start the node.
    ///
    /// Begins syncing the blockchain, serving the Electrum and JSON-RPC
    /// interfaces, and watching configured wallets. Returns an error if
    /// initialization fails.
    pub fn start(&self) -> Result<(), FlorestaFfiError> {
        self.rt.block_on(async {
            self.florestad
                .start()
                .await
                .map_err(|e| FlorestaFfiError::StartError {
                    details: e.to_string(),
                })
        })
    }

    /// Gracefully stop the node.
    ///
    /// Waits for all pending operations to finish and flushes data to disk.
    /// Always call this before exiting to avoid data corruption.
    pub fn stop(&self) {
        self.rt.block_on(async {
            self.florestad.stop().await;
        });
    }

    /// Dump the current Utreexo accumulator as a portable JSON snapshot.
    ///
    /// The payload is suitable for transfer to another device (clipboard, QR,
    /// share sheet) and is safe to make public — it contains only consensus
    /// data, never wallet descriptors or xpubs. Returns
    /// [`UtreexoExportError::NotStarted`] if the daemon has not been started
    /// yet, [`UtreexoExportError::NotSynced`] while IBD is in progress.
    pub fn dump_utreexo_state(&self) -> Result<String, UtreexoExportError> {
        let snapshot = self.florestad.dump_utreexo_state().map_err(|e| match e {
            floresta_node::DumpError::NotStarted => UtreexoExportError::NotStarted,
            floresta_node::DumpError::NotSynced => UtreexoExportError::NotSynced,
            floresta_node::DumpError::Chain(_) => UtreexoExportError::Internal,
        })?;
        Ok(snapshot.to_json(self.network))
    }
}

impl Default for Florestad {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for the Floresta daemon.
pub struct Config {
    /// Path to the data directory. Must be readable and writable.
    pub datadir: String,

    /// The Bitcoin network to run on.
    pub network: Network,

    /// Disable DNS seed nodes for peer discovery.
    pub disable_dns_seeds: bool,

    /// Which blocks are assumed to have valid scripts.
    pub assume_valid: AssumeValidArg,

    /// SLIP-132-encoded extended public keys to watch.
    pub wallet_xpub: Option<Vec<String>>,

    /// Output descriptors to watch.
    pub wallet_descriptor: Option<Vec<String>>,

    /// Path to a TOML configuration file.
    pub config_file: Option<String>,

    /// SOCKS5 proxy for outgoing connections.
    pub proxy: Option<String>,

    /// Whether to build compact block filters.
    pub cfilters: bool,

    /// Block height to start downloading compact filters from.
    pub filters_start_height: Option<i32>,

    /// ZMQ server address (requires zmq-server feature).
    pub zmq_address: Option<String>,

    /// Nodes to connect to exclusively.
    pub connect: Vec<String>,

    /// JSON-RPC server address (requires json-rpc feature).
    pub json_rpc_address: Option<String>,

    /// Whether to write logs to stdout.
    pub log_to_stdout: bool,

    /// Whether to write logs to a file.
    pub log_to_file: bool,

    /// Enable assume-utreexo mode.
    pub assume_utreexo: bool,

    /// Enable debug logging.
    pub debug: bool,

    /// User agent string advertised to peers.
    pub user_agent: String,

    /// Custom Utreexo accumulator state for assume-utreexo.
    pub assumeutreexo_value: Option<AssumeUtreexoValue>,

    /// Electrum server address.
    pub electrum_address: Option<String>,

    /// Whether to enable the Electrum TLS server.
    pub enable_electrum_tls: bool,

    /// Electrum TLS server address.
    pub electrum_address_tls: Option<String>,

    /// Path to the TLS private key file.
    pub tls_key_path: Option<String>,

    /// Path to the TLS certificate file.
    pub tls_cert_path: Option<String>,

    /// Whether to generate a self-signed TLS certificate.
    pub generate_cert: bool,

    /// Whether to allow v1 transport fallback.
    pub allow_v1_fallback: bool,

    /// Whether to backfill skipped blocks.
    pub backfill: bool,
}

impl From<Config> for floresta_node::Config {
    fn from(config: Config) -> floresta_node::Config {
        let mut cfg =
            floresta_node::Config::new(config.network.into(), PathBuf::from(&config.datadir));

        cfg.disable_dns_seeds = config.disable_dns_seeds;
        cfg.wallet_xpub = config.wallet_xpub;
        cfg.wallet_descriptor = config.wallet_descriptor;
        cfg.config_file = config.config_file.map(PathBuf::from);
        cfg.proxy = config.proxy;
        cfg.cfilters = config.cfilters;
        cfg.filters_start_height = config.filters_start_height;
        // The mandacaru node's `connect` is a single optional address, not a
        // list; take the first entry if the caller supplied any.
        cfg.connect = config.connect.into_iter().next();
        cfg.json_rpc_address = config.json_rpc_address;

        #[cfg(feature = "zmq-server")]
        {
            cfg.zmq_address = config.zmq_address;
        }

        cfg.log_to_stdout = config.log_to_stdout;
        cfg.log_to_file = config.log_to_file;
        cfg.assume_utreexo = config.assume_utreexo;
        cfg.debug = config.debug;
        cfg.user_agent = config.user_agent;
        cfg.electrum_address = config.electrum_address;
        cfg.enable_electrum_tls = config.enable_electrum_tls;
        cfg.electrum_address_tls = config.electrum_address_tls;
        cfg.tls_key_path = config.tls_key_path.map(PathBuf::from);
        cfg.tls_cert_path = config.tls_cert_path.map(PathBuf::from);
        cfg.generate_cert = config.generate_cert;
        cfg.allow_v1_fallback = config.allow_v1_fallback;
        cfg.backfill = config.backfill;

        cfg.assume_valid = match config.assume_valid {
            AssumeValidArg::Disabled => floresta_node::AssumeValidArg::Disabled,
            AssumeValidArg::Hardcoded => floresta_node::AssumeValidArg::Hardcoded,
            AssumeValidArg::UserInput { block_hash } => {
                let hash = bitcoin::BlockHash::from_str(&block_hash)
                    .unwrap_or_else(|_| bitcoin::BlockHash::all_zeros());
                floresta_node::AssumeValidArg::UserInput(hash)
            }
        };

        cfg.assumeutreexo_value = config.assumeutreexo_value.and_then(|v| {
            let hash = bitcoin::BlockHash::from_str(&v.block_hash)
                .ok()
                .unwrap_or_else(bitcoin::BlockHash::all_zeros);
            let roots: Vec<rustreexo::node_hash::BitcoinNodeHash> = v
                .roots
                .iter()
                .filter_map(|r| rustreexo::node_hash::BitcoinNodeHash::from_str(r).ok())
                .collect();
            if roots.len() != v.roots.len() {
                return None;
            }
            Some(floresta_node::AssumeUtreexoValue {
                block_hash: hash,
                height: v.height,
                roots,
                leaves: v.leaves,
            })
        });

        cfg
    }
}

/// Verify that `payload` is a well-formed snapshot produced by
/// [`Florestad::dump_utreexo_state`] and is for `expected_network`.
///
/// Use this before triggering a restart-to-import on the Android side so a
/// bad QR / paste does not reach [`Florestad::from_config`].
pub fn validate_utreexo_snapshot_json(
    payload: String,
    expected_network: Network,
) -> Result<(), UtreexoImportError> {
    let expected: bitcoin::Network = expected_network.into();
    let (_, got) =
        floresta_node::UtreexoSnapshot::from_json(&payload).map_err(map_snapshot_error)?;
    if got != expected {
        return Err(UtreexoImportError::NetworkMismatch);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Utreexo snapshot error types surfaced through UniFFI.
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum UtreexoExportError {
    NotStarted,
    NotSynced,
    Internal,
}

impl std::fmt::Display for UtreexoExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UtreexoExportError::NotStarted => write!(f, "daemon not started yet"),
            UtreexoExportError::NotSynced => write!(f, "initial block download not finished"),
            UtreexoExportError::Internal => write!(f, "internal chain error while dumping"),
        }
    }
}

impl std::error::Error for UtreexoExportError {}

#[derive(Debug)]
pub enum UtreexoImportError {
    InvalidJson,
    UnsupportedVersion,
    UnknownNetwork,
    InvalidHex,
    NetworkMismatch,
}

impl std::fmt::Display for UtreexoImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UtreexoImportError::InvalidJson => write!(f, "payload is not valid snapshot JSON"),
            UtreexoImportError::UnsupportedVersion => {
                write!(f, "snapshot version is not supported by this build")
            }
            UtreexoImportError::UnknownNetwork => write!(f, "snapshot network tag is unknown"),
            UtreexoImportError::InvalidHex => write!(f, "snapshot hex field failed to parse"),
            UtreexoImportError::NetworkMismatch => {
                write!(f, "snapshot is for a different network than this node")
            }
        }
    }
}

impl std::error::Error for UtreexoImportError {}

fn map_snapshot_error(e: floresta_node::SnapshotError) -> UtreexoImportError {
    match e {
        floresta_node::SnapshotError::InvalidJson(_) => UtreexoImportError::InvalidJson,
        floresta_node::SnapshotError::UnsupportedVersion(_) => {
            UtreexoImportError::UnsupportedVersion
        }
        floresta_node::SnapshotError::UnknownNetwork(_) => UtreexoImportError::UnknownNetwork,
        floresta_node::SnapshotError::InvalidHex(_) => UtreexoImportError::InvalidHex,
        floresta_node::SnapshotError::NetworkMismatch { .. } => UtreexoImportError::NetworkMismatch,
    }
}
