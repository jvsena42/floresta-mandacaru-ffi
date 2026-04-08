uniffi::include_scaffolding!("floresta");

use bitcoin::Network;

/// The FFI representation of the florestad service
///
/// This struct holds florestad and a runtime that will be used to run the service. It is the
/// public interface to the service, and it should be used to interact with it.
pub struct Florestad {
    rt: tokio::runtime::Runtime,
    florestad: floresta_node::Florestad,
    _log_guard: Option<floresta_node::WorkerGuard>,
}

impl Florestad {
    /// Create a new instance of florestad
    ///
    /// This will create a new instance of florestad, but it won't start it. You need to call
    /// `start` to start the service. We'll use the default configuration for the service.
    pub fn new() -> Florestad {
        let _rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .thread_name("florestad")
            .build()
            .unwrap();

        let data_dir = String::from("/data/data/com.github.jvsena42.mandacaru/files");
        let log_guard = floresta_node::init_logging(&data_dir, true, false, false)
            .ok()
            .flatten();
        let florestad =
            floresta_node::Florestad::new(bitcoin::Network::Bitcoin, data_dir);
        Self { rt: _rt, florestad, _log_guard: log_guard }
    }
    
    /// Create a new instance of florestad from a configuration
    ///
    /// This will create a new instance of florestad, but it won't start it. You need to call
    /// `start` to start the service. We'll use the configuration provided to start the service.
    pub fn from_config(config: Config) -> Florestad {
        let _rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .thread_name("florestad")
            .build()
            .unwrap();

        let node_config: floresta_node::Config = config.into();
        let log_guard = floresta_node::init_logging(
            &node_config.data_dir,
            node_config.log_to_file,
            node_config.log_to_stdout,
            node_config.debug,
        )
        .ok()
        .flatten();
        let florestad = floresta_node::Florestad::from_config(node_config);
        Self { rt: _rt, florestad, _log_guard: log_guard }
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
}

impl From<Config> for floresta_node::Config {
    fn from(config: Config) -> floresta_node::Config {
        let data_dir = config
            .data_dir
            .unwrap_or_else(|| String::from("/data/data/com.github.jvsena42.mandacaru/files"));

        let mut node_config = floresta_node::Config::new(config.network, data_dir);
        node_config.electrum_address = config.electrum_address;
        node_config.json_rpc_address = config.rpc_address;
        node_config.filters_start_height = config.filters_start_height;
        node_config.log_to_file = true;
        node_config.log_to_stdout = false;
        node_config.assume_utreexo = false;
        node_config.wallet_descriptor = config.wallet_descriptor.map(|desc| vec![desc]);
        node_config.cfilters = true;
        node_config
    }
}
