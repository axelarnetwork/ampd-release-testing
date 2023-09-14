use std::fmt::Debug;
use std::fs::canonicalize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ::config::{Config as cfg, Environment, File, FileFormat, FileSourceFile};
use clap::{command, Parser, Subcommand, ValueEnum};
use config::ConfigError;
use cosmrs::{AccountId, Coin};
use error_stack::{Report, ResultExt};
use tracing::{error, info};
use valuable::Valuable;

use ampd::cli;
use ampd::cli::{BondWorkerArgs, DeclareChainSupportArgs};
use ampd::config::Config;
use ampd::report::Error;
use ampd::report::LoggableError;
use ampd::run;
use axelar_wasm_std::utils::InspectorResult;
use axelar_wasm_std::FnExt;

#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Set the paths for config file lookup. Can be defined multiple times (configs get merged)
    #[arg(short, long, default_values_os_t = vec![std::path::PathBuf::from("~/.ampd/config.toml"), std::path::PathBuf::from("config.toml")])]
    pub config: Vec<PathBuf>,

    /// Set the paths for state file lookup
    #[arg(short, long, default_value_os_t = std::path::PathBuf::from("~/.ampd/state.json"))]
    pub state: PathBuf,

    /// Set the output style of the logs
    #[arg(short, long, value_enum, default_value_t = Output::Json)]
    pub output: Output,

    #[clap(subcommand)]
    pub cmd: Option<SubCommand>,
}

#[derive(Debug, Clone, Parser, ValueEnum, Valuable)]
enum Output {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
enum SubCommand {
    /// Run the ampd daemon process (default)
    Daemon,
    /// Bond worker to a service
    BondWorker(BondWorkerArgs),
    /// Declare chain support for a service
    DeclareChainSupport(DeclareChainSupportArgs),
    /// Register worker public key to the multisig signer
    RegisterPublicKey,
    WorkerAddress,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Args = Args::parse();
    set_up_logger(&args.output);

    match &args.cmd {
        Some(SubCommand::Daemon) | None =>{
            let result = run_daemon(&args)
                .await
                .tap_err(|report| error!(err = LoggableError::from(report).as_value(), "{report}"));
            info!("shutting down");
            match result {
                Ok(_) => ExitCode::SUCCESS,
                Err(report) => {
                    // print detailed error report as the last output if in text mode
                    if matches!(args.output, Output::Text) {
                        eprintln!("{report:?}");
                    }

                    ExitCode::FAILURE
                }
            }

        },
        Some(SubCommand::BondWorker(cmd_args)) => bond_worker(&args, cmd_args).await,
        Some(SubCommand::DeclareChainSupport(cmd_args)) => {
            declare_chain_support(&args, cmd_args).await
        }
        Some(SubCommand::RegisterPublicKey) => register_public_key(&args).await,
        Some(SubCommand::WorkerAddress) => worker_address(&args).await,
    }
}

async fn bond_worker(args: &Args, params: &BondWorkerArgs) -> ExitCode {
    info!("registering worker");

    let cfg = init_config(&args.config);
    let coin = Coin::new(params.amount, params.denom.as_str()).unwrap();
    let service_registry = params.service_registry.parse::<AccountId>().unwrap();

    cli::bond_worker(
        cfg,
        args.state.clone(),
        service_registry,
        params.service_name.clone(),
        coin,
    )
    .await;

    ExitCode::SUCCESS
}

async fn declare_chain_support(args: &Args, params: &DeclareChainSupportArgs) -> ExitCode {
    info!("declaring chain support");

    let cfg = init_config(&args.config);
    let service_registry = params.service_registry.parse::<AccountId>().unwrap();

    cli::declare_chain_support(
        cfg,
        args.state.clone(),
        service_registry,
        params.service_name.clone(),
        params.chains.clone(),
    )
    .await;

    ExitCode::SUCCESS
}

async fn register_public_key(args: &Args) -> ExitCode {
    info!("registering public key to multisig signer contract");

    let cfg = init_config(&args.config);
    cli::register_public_key(cfg, args.state.clone()).await;

    ExitCode::SUCCESS
}

async fn worker_address(args: &Args) -> ExitCode {
    info!("querying worker address");

    let cfg = init_config(&args.config);
    cli::worker_address(cfg, args.state.clone()).await;

    ExitCode::SUCCESS
}


fn set_up_logger(output: &Output) {
    match output {
        Output::Json => {
            tracing_subscriber::fmt().json().flatten_event(true).init();
        }
        Output::Text => {
            tracing_subscriber::fmt().compact().init();
        }
    };
}

async fn run_daemon(args: &Args) -> Result<(), Report<Error>> {
    let cfg = init_config(&args.config);
    let state_path = expand_home_dir(args.state.as_path());

    run(cfg, state_path).await
}

fn init_config(config_paths: &[PathBuf]) -> Config {
    let files = find_config_files(config_paths);

    parse_config(files)
        .change_context(Error::LoadConfig)
        .tap_err(|report| error!(err = LoggableError::from(report).as_value(), "{report}"))
        .unwrap_or(Config::default())
}

fn find_config_files(config: &[PathBuf]) -> Vec<File<FileSourceFile, FileFormat>> {
    let files = config
        .iter()
        .map(PathBuf::as_path)
        .map(expand_home_dir)
        .map(canonicalize)
        .filter_map(Result::ok)
        .inspect(|path| info!("found config file {}", path.to_string_lossy()))
        .map(File::from)
        .collect::<Vec<_>>();

    if files.is_empty() {
        info!("found no config files to load");
    }

    files
}

fn parse_config(
    files: Vec<File<FileSourceFile, FileFormat>>,
) -> error_stack::Result<Config, ConfigError> {
    cfg::builder()
        .add_source(files)
        .add_source(Environment::with_prefix(clap::crate_name!()))
        .build()?
        .try_deserialize::<Config>()
        .map_err(Report::from)
}

fn expand_home_dir(path: &Path) -> PathBuf {
    let Ok(home_subfolder) = path.strip_prefix("~") else{
        return path.to_path_buf()
    };

    dirs::home_dir().map_or(path.to_path_buf(), |home| home.join(home_subfolder))
}
