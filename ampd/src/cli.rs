use std::convert::TryFrom;
use std::path::PathBuf;

use clap::Args;
use cosmos_sdk_proto::cosmos::{
    auth::v1beta1::query_client::QueryClient, tx::v1beta1::service_client::ServiceClient,
};
use cosmrs::cosmwasm::MsgExecuteContract;
use cosmrs::tx::Msg;
use cosmrs::{AccountId, Coin};
use error_stack::ResultExt;

use multisig::{
    key::{KeyType, PublicKey as MultisigPublicKey},
    msg::ExecuteMsg as MultisigExecuteMsg,
};
use service_registry::msg::ExecuteMsg;

use crate::broadcaster;
use crate::broadcaster::{accounts::account, Broadcaster, Config as BroadcastConfig};
use crate::config::Config;
use crate::handlers;
use crate::report::Error;
use crate::state::StateUpdater;
use crate::tofnd::grpc::{MultisigClient, SharableEcdsaClient};
use crate::types::PublicKey;
use crate::url::Url;

type Result<T> = error_stack::Result<T, Error>;

const PREFIX: &str = "axelar";

#[derive(Args, Debug)]
pub struct BondWorkerArgs {
    pub service_registry: String,
    pub service_name: String,
    pub amount: u128,
    pub denom: String,
}

#[derive(Args, Debug)]
pub struct DeclareChainSupportArgs {
    pub service_registry: String,
    pub service_name: String,
    pub chains: Vec<String>,
}

pub async fn bond_worker(
    config: Config,
    state_path: PathBuf,
    service_registry: AccountId,
    service_name: String,
    coin: Coin,
) {
    let Config {
        tm_grpc,
        broadcast,
        tofnd_config,
        ..
    } = config;

    let multisig_client = MultisigClient::connect(tofnd_config.party_uid, tofnd_config.url)
        .await
        .change_context(Error::Connection)
        .unwrap();

    let ecdsa_client = SharableEcdsaClient::new(multisig_client);

    let pub_key = pub_key(
        state_path,
        tofnd_config.key_uid.as_str(),
        ecdsa_client.clone(),
    )
    .await
    .unwrap();

    let msg = serde_json::to_vec(&ExecuteMsg::BondWorker { service_name })
        .expect("bond worker msg should serialize");

    let tx = MsgExecuteContract {
        sender: pub_key
            .account_id(PREFIX)
            .expect("failed to convert to account identifier"),
        contract: service_registry,
        msg,
        funds: vec![coin],
    };

    broadcast_execute_contract(
        tm_grpc,
        broadcast,
        tofnd_config.key_uid,
        tx,
        pub_key,
        ecdsa_client,
    )
    .await
}

pub async fn declare_chain_support(
    config: Config,
    state_path: PathBuf,
    service_registry: AccountId,
    service_name: String,
    chains: Vec<String>,
) {
    let Config {
        tm_grpc,
        broadcast,
        tofnd_config,
        ..
    } = config;

    let multisig_client = MultisigClient::connect(tofnd_config.party_uid, tofnd_config.url)
        .await
        .change_context(Error::Connection)
        .unwrap();

    let ecdsa_client = SharableEcdsaClient::new(multisig_client);

    let pub_key = pub_key(
        state_path,
        tofnd_config.key_uid.as_str(),
        ecdsa_client.clone(),
    )
    .await
    .unwrap();

    let msg = serde_json::to_vec(&ExecuteMsg::DeclareChainSupport {
        service_name,
        chains,
    })
    .expect("declare chain support msg should serialize");

    let tx = MsgExecuteContract {
        sender: pub_key
            .account_id(PREFIX)
            .expect("failed to convert to account identifier"),
        contract: service_registry,
        msg,
        funds: vec![],
    };

    broadcast_execute_contract(
        tm_grpc,
        broadcast,
        tofnd_config.key_uid,
        tx,
        pub_key,
        ecdsa_client,
    )
    .await
}

pub async fn register_public_key(config: Config, state_path: PathBuf) {
    let Config {
        tm_grpc,
        broadcast,
        tofnd_config,
        handlers,
        ..
    } = config;

    let multisig_client = MultisigClient::connect(tofnd_config.party_uid, tofnd_config.url)
        .await
        .change_context(Error::Connection)
        .unwrap();

    let ecdsa_client = SharableEcdsaClient::new(multisig_client);

    let pub_key = pub_key(
        state_path,
        tofnd_config.key_uid.as_str(),
        ecdsa_client.clone(),
    )
    .await
    .unwrap();

    // get multisig contract address
    let multisig_address = handlers
        .iter()
        .filter_map(|h| {
            if let handlers::config::Config::MultisigSigner { cosmwasm_contract } = h {
                Some(cosmwasm_contract.clone())
            } else {
                None
            }
        })
        .next()
        .expect("No multisig signer found in handlers");

    // get tofnd pub key
    let multisig_pub_key = ecdsa_client
        .keygen(multisig_address.to_string().as_str())
        .await
        .unwrap();
    let multisig_pub_key =
        MultisigPublicKey::try_from((KeyType::Ecdsa, multisig_pub_key.to_bytes().into())).unwrap();
    let msg = serde_json::to_vec(&MultisigExecuteMsg::RegisterPublicKey {
        public_key: multisig_pub_key,
    })
    .expect("bond worker msg should serialize");

    let tx = MsgExecuteContract {
        sender: pub_key
            .account_id(PREFIX)
            .expect("failed to convert to account identifier"),
        contract: multisig_address.to_string().as_str().parse().unwrap(),
        msg,
        funds: vec![],
    };

    broadcast_execute_contract(
        tm_grpc,
        broadcast,
        tofnd_config.key_uid,
        tx,
        pub_key,
        ecdsa_client,
    )
    .await
}

pub async fn worker_address(config: Config, state_path: PathBuf) {
    let Config {
        tofnd_config,
        ..
    } = config;

    let multisig_client = MultisigClient::connect(tofnd_config.party_uid, tofnd_config.url)
        .await
        .change_context(Error::Connection)
        .unwrap();

    let ecdsa_client = SharableEcdsaClient::new(multisig_client);

    let pub_key = pub_key(
        state_path,
        tofnd_config.key_uid.as_str(),
        ecdsa_client.clone(),
    )
    .await
    .unwrap();

    println!(
        "Worker address is {}",
        pub_key
            .account_id(PREFIX)
            .expect("failed to convert to account identifier")
            .to_string()
    )
}

async fn pub_key(
    state_path: PathBuf,
    key_uid: &str,
    ecdsa_client: SharableEcdsaClient,
) -> Result<PublicKey> {
    let mut state_updater = StateUpdater::new(state_path).change_context(Error::StateUpdater)?;

    match state_updater.state().pub_key {
        Some(pub_key) => Ok(pub_key),
        None => {
            let pub_key = ecdsa_client
                .keygen(key_uid)
                .await
                .change_context(Error::Tofnd)?;
            state_updater.as_mut().pub_key = Some(pub_key);

            Ok(pub_key)
        }
    }
}

async fn broadcast_execute_contract(
    tm_grpc: Url,
    broadcast: BroadcastConfig,
    key_uid: String,
    tx: MsgExecuteContract,
    pub_key: PublicKey,
    ecdsa_client: SharableEcdsaClient,
) {
    let query_client = QueryClient::connect(tm_grpc.to_string())
        .await
        .change_context(Error::Connection)
        .unwrap();

    let worker = pub_key
        .account_id(PREFIX)
        .expect("failed to convert to account identifier")
        .into();
    let account = account(query_client, &worker)
        .await
        .change_context(Error::Broadcaster)
        .unwrap();

    let service_client = ServiceClient::connect(tm_grpc.to_string())
        .await
        .change_context(Error::Connection)
        .unwrap();

    let mut broadcaster = broadcaster::BroadcastClientBuilder::default()
        .client(service_client)
        .signer(ecdsa_client.clone())
        .acc_number(account.account_number)
        .acc_sequence(account.sequence)
        .pub_key((key_uid, pub_key))
        .config(broadcast.clone())
        .build()
        .change_context(Error::Broadcaster)
        .unwrap();

    broadcaster
        .broadcast(vec![tx.into_any().unwrap()])
        .await
        .unwrap();
}
