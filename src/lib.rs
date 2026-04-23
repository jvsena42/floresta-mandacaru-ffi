uniffi::include_scaffolding!("floresta");

use bitcoin::Network;

/// Default data directory when the caller does not supply one.
///
/// The on-disk layout here is chosen to match the Android app's per-package
/// data directory on the installed device.
const DEFAULT_DATA_DIR: &str = "/data/data/com.github.jvsena42.mandacaru/files";

/// The FFI representation of the florestad service
///
/// This struct holds florestad and a runtime that will be used to run the service. It is the
/// public interface to the service, and it should be used to interact with it.
pub struct Florestad {
    rt: tokio::runtime::Runtime,
    florestad: floresta_node::Florestad,
    /// The network this daemon was constructed for. Stamped into
    /// `dump_utreexo_state` JSON payloads so the receiver can refuse cross-network
    /// imports.
    network: Network,
    _log_guard: Option<floresta_node::WorkerGuard>,
}

impl Florestad {
    /// Create a new instance of florestad
    ///
    /// This will create a new instance of florestad, but it won't start it. You need to call
    /// `start` to start the service. We'll use the default configuration for the service.
    pub fn new() -> Florestad {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .thread_name("florestad")
            .build()
            .unwrap();

        let data_dir = String::from(DEFAULT_DATA_DIR);
        let log_guard = floresta_node::init_logging(&data_dir, true, false, false)
            .ok()
            .flatten();
        let network = bitcoin::Network::Bitcoin;
        let florestad = floresta_node::Florestad::new(network, data_dir);
        Self { rt, florestad, network, _log_guard: log_guard }
    }

    /// Create a new instance of florestad from a configuration
    ///
    /// This will create a new instance of florestad, but it won't start it. You need to call
    /// `start` to start the service. We'll use the configuration provided to start the service.
    ///
    /// # Panics
    ///
    /// Panics if `config.user_utreexo_snapshot_json` is set but malformed or on the wrong network.
    /// Callers should validate the payload first with [`validate_utreexo_snapshot_json`].
    pub fn from_config(config: Config) -> Florestad {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .thread_name("florestad")
            .build()
            .unwrap();

        let node_config: floresta_node::Config = config.into();
        let network = node_config.network;
        let log_guard = floresta_node::init_logging(
            &node_config.data_dir,
            node_config.log_to_file,
            node_config.log_to_stdout,
            node_config.debug,
        )
        .ok()
        .flatten();
        // Anything logged before `init_logging` returns is dropped because the
        // tracing subscriber isn't installed yet. Emit the snapshot summary
        // here, once the subscriber is live, by inspecting the already-converted
        // node_config instead.
        match &node_config.assumeutreexo_value {
            Some(av) => tracing::info!(
                target: "floresta_ffi",
                "applying user_utreexo_snapshot_json: height={} leaves={} roots={} block_hash={}",
                av.height,
                av.leaves,
                av.roots.len(),
                av.block_hash,
            ),
            None => tracing::info!(
                target: "floresta_ffi",
                "no user_utreexo_snapshot_json; fastSync flag assume_utreexo={}",
                node_config.assume_utreexo,
            ),
        }
        let florestad = floresta_node::Florestad::from_config(node_config);
        Self { rt, florestad, network, _log_guard: log_guard }
    }

    /// Gracefully stop the service
    ///
    /// This method should be called before your application exits. It will stop the service
    /// gracefully, waiting for all pending requests to finish. If you don't call this method, you
    /// may corrupt the data and lose data.
    ///
    /// This is equivalent to calling the `stop` rpc method.
    pub fn stop(&self) {
        self.rt.block_on(async {
            self.florestad.stop().await;
        });
    }

    /// Start the service
    ///
    /// This method will start the service. It will block the current thread until the service is
    /// fully started. It should not take long, but you may prefer calling it outside of the main
    /// thread.
    ///
    /// After this function returns, the service is ready to accept requests. Floresta will start
    /// running on the background and do all the heavy lifting for you.
    pub fn start(&self) {
        self.rt.block_on(async {
            self.florestad.start().await.expect("Failed to start florestad");
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
        let snapshot = self
            .florestad
            .dump_utreexo_state()
            .map_err(|e| match e {
                floresta_node::DumpError::NotStarted => UtreexoExportError::NotStarted,
                floresta_node::DumpError::NotSynced => UtreexoExportError::NotSynced,
                floresta_node::DumpError::Chain(_) => UtreexoExportError::Internal,
            })?;
        Ok(snapshot.to_json(self.network))
    }
}

pub struct Config {
    /// Where we should place our data
    ///
    /// This directory must be readable and writable by our proccess. We'll use this dir to store
    /// both chain and wallet data, so this should be kept in a non-volatile medium. We are not
    /// particurly aggressive in disk usage, so we don't need a fast disk to work.
    ///
    /// If not set, it defaults to $HOME/.floresta
    pub data_dir: Option<String>,

    /// The address of the electrum server to listen on
    pub electrum_address: Option<String>,

    /// The address of the json rpc server to listen on
    pub rpc_address: Option<String>,

    /// The height of the first filter we should download
    pub filters_start_height: Option<i32>,

    /// The network we are running on
    pub network: Network,

    /// The wallet xpub to keep track of funds for
    pub wallet_xpub: Option<String>,

    /// The wallet descriptor to keep track of funds for
    pub wallet_descriptor: Option<String>,

    /// Whether we should use assume utreexo
    pub assume_utreexo: bool,

    /// JSON payload produced by [`Florestad::dump_utreexo_state`] on another
    /// synced node. When present, the daemon uses it instead of the hardcoded
    /// assume-utreexo checkpoint. Validated at `From<Config>` time — a bad
    /// payload will panic (callers should pre-check with
    /// [`validate_utreexo_snapshot_json`]).
    pub user_utreexo_snapshot_json: Option<String>,
}

impl From<Config> for floresta_node::Config {
    fn from(config: Config) -> floresta_node::Config {
        let data_dir = config
            .data_dir
            .unwrap_or_else(|| String::from(DEFAULT_DATA_DIR));

        let mut node_config = floresta_node::Config::new(config.network, data_dir);
        node_config.electrum_address = config.electrum_address;
        node_config.json_rpc_address = config.rpc_address;
        node_config.filters_start_height = config.filters_start_height;
        node_config.log_to_file = true;
        node_config.log_to_stdout = false;
        node_config.assume_utreexo = config.assume_utreexo;
        node_config.wallet_descriptor = config.wallet_descriptor.map(|desc| vec![desc]);
        node_config.cfilters = true;

        if let Some(payload) = config.user_utreexo_snapshot_json {
            let snap = floresta_node::UtreexoSnapshot::from_json(&payload)
                .expect("user_utreexo_snapshot_json failed to parse — validate it first");
            if snap.1 != config.network {
                panic!(
                    "user_utreexo_snapshot_json is for a different network than Config.network"
                );
            }
            node_config.assumeutreexo_value = Some(snap.0.into_assume_value());
        }

        node_config
    }
}

/// Verify that `payload` is a well-formed snapshot produced by
/// [`Florestad::dump_utreexo_state`] and is for `expected_network`.
///
/// Use this before triggering a restart-to-import on the Android side so a
/// bad QR / paste does not reach [`Florestad::from_config`] (which panics).
pub fn validate_utreexo_snapshot_json(
    payload: String,
    expected_network: Network,
) -> Result<(), UtreexoImportError> {
    let (_, got) =
        floresta_node::UtreexoSnapshot::from_json(&payload).map_err(map_snapshot_error)?;
    if got != expected_network {
        return Err(UtreexoImportError::NetworkMismatch);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Error types surfaced through UniFFI.
//
// UniFFI 0.26 requires error types to implement `std::fmt::Display` and
// `std::error::Error`. Keep these as simple unit variants — the UDL mirrors
// them verbatim.
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
        floresta_node::SnapshotError::UnsupportedVersion(_) => UtreexoImportError::UnsupportedVersion,
        floresta_node::SnapshotError::UnknownNetwork(_) => UtreexoImportError::UnknownNetwork,
        floresta_node::SnapshotError::InvalidHex(_) => UtreexoImportError::InvalidHex,
        floresta_node::SnapshotError::NetworkMismatch { .. } => UtreexoImportError::NetworkMismatch,
    }
}
