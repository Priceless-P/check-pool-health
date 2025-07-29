pub mod errors;
mod jd_connection;
mod mining_pool_connection;
mod utils;

use crate::jd_connection::SetupConnectionHandler;
use crate::mining_pool_connection::{
    initialize_mining_connections, mining_setup_connection, open_channel, relay_down, relay_up,
};
use crate::utils::AbortOnDrop;
use binary_sv2::{Seq064K, Sv2DataType, B0255, B064K};
use codec_sv2::{Frame, HandshakeRole, StandardEitherFrame, StandardSv2Frame};
use demand_share_accounting_ext::parser::PoolExtMessages;
use demand_sv2_connection::noise_connection_tokio::Connection;
use key_utils::Secp256k1PublicKey;
use noise_sv2::Initiator;
use roles_logic_sv2::{
    job_declaration_sv2::{AllocateMiningJobToken, DeclareMiningJob},
    parsers::{JobDeclaration, Mining, PoolMessages},
};
use std::{
    collections::HashSet,
    net::{SocketAddr, ToSocketAddrs},
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::mpsc::{channel, Receiver, Sender},
    time::{sleep, Instant},
};

pub type Message = PoolExtMessages<'static>;
pub type Msg = PoolMessages<'static>;
pub type StdFrame = StandardSv2Frame<Message>;
pub type SFrame = StandardSv2Frame<Msg>;
pub type EitherFrame = StandardEitherFrame<Message>;

const TIMEOUT: Duration = Duration::from_secs(10);

const AUTH_PUB_KEY: &str = "9auqWEzQDVyd2oe1JVGFLMLHZtCo2FFqZwtKA5gd9xbuEu7PH72";

// List of messages we expect to receive to confirm the pool is healthy
#[derive(Debug, PartialEq, Eq, Hash)]
enum ExpectedMessage {
    SetupConnectionSuccess,
    OpenExtendedMiningChannelSuccess,
    NewExtendedMiningJob,
    SetNewPrevHash,
    AllocateMiningJobTokenSuccess,
    DeclareMiningJobSuccess,
}

/// Checks if the pool is healthy by establishing a connection and verifying the expected messages are received.
///
/// ### Args
/// - `pool`: The address of the pool.
/// - `token`: The token of the user.
/// - `auth_key`: Optional AUTH_PUB_KEY for the connection.
///   If `auth_key` is not provided, the default AUTH_PUB_KEY is used.
///
/// ### Returns
/// Prints the status of the pool connection and returns an error if the expected messages are not received within the timeout period.
pub async fn check(pool: String, token: String, auth_key: Option<String>) -> Result<(), String> {
    // Create a channel for to ExpectedMessage
    let (msg_tx, msg_rx) = channel::<ExpectedMessage>(10);
    let pool: SocketAddr = match pool.to_socket_addrs() {
        Ok(mut addr) => addr.next().expect("Could not resolve pool address"),
        Err(_) => return Err("Invalid pool address".into()),
    };
    let default_auth_key: Secp256k1PublicKey = AUTH_PUB_KEY.parse().expect("Invalid public key");
    let auth_key = match auth_key {
        Some(key) => key.parse().map_err(|_| "Invalid auth key".to_string())?,
        None => default_auth_key,
    };

    // Set up a connection to the pool
    let (send_from_down, recv_from_down, relay_up_task, relay_down_task) =
        setup_mining_connection(pool, auth_key, msg_tx.clone(), token.clone()).await;

    if send_from_down
        .send(PoolExtMessages::Mining(open_channel()))
        .await
        .is_err()
    {
        return Err("Failed to send OpenExtendedMiningChannel".into());
    }

    // Set up a connection for jd
    let (jd_sender, jd_receiver) = setup_jd_connection(pool, auth_key, token).await;

    let pool_messages = tokio::spawn(handle_pool_messages(recv_from_down, msg_tx.clone()));
    let jd_messages = tokio::spawn(handle_jd_messages(jd_receiver, jd_sender.clone(), msg_tx));

    let result = await_messages(msg_rx).await;

    pool_messages.abort();
    jd_messages.abort();
    drop(relay_up_task);
    drop(relay_down_task);

    result
}

async fn setup_mining_connection(
    pool: SocketAddr,
    auth_key: Secp256k1PublicKey,
    msg_tx: Sender<ExpectedMessage>,
    token: String,
) -> (
    Sender<PoolExtMessages<'static>>,
    Receiver<PoolExtMessages<'static>>,
    AbortOnDrop,
    AbortOnDrop,
) {
    println!("Checking pool at {}....", pool);
    let stream = TcpStream::connect(pool).await.expect("Connection failed");

    let (mut receiver, mut sender, setup_msg) =
        initialize_mining_connections(None, stream, auth_key, token)
            .await
            .expect("Failed to init mining conn");

    if mining_setup_connection(
        &mut receiver,
        &mut sender,
        setup_msg,
        Duration::from_secs(5),
    )
    .await
    .is_ok()
    {
        if msg_tx
            .send(ExpectedMessage::SetupConnectionSuccess)
            .await
            .is_err()
        {
            eprintln!("Failed to send ExpectedMessage");
        }
    } else {
        eprintln!("Failed to setup mining connection")
    };

    let (send_to_down, recv_from_down) = tokio::sync::mpsc::channel(100);
    let (send_from_down, recv_to_up) = tokio::sync::mpsc::channel(100);
    let relay_up_task = relay_up(recv_to_up, sender.clone());
    let relay_down_task = relay_down(receiver, send_to_down);

    (
        send_from_down,
        recv_from_down,
        relay_up_task,
        relay_down_task,
    )
}

async fn setup_jd_connection(
    pool: SocketAddr,
    auth_key: Secp256k1PublicKey,
    token: String,
) -> (
    Sender<Frame<PoolMessages<'static>, codec_sv2::buffer_sv2::Slice>>,
    Receiver<Frame<PoolMessages<'static>, codec_sv2::buffer_sv2::Slice>>,
) {
    let jd_stream = TcpStream::connect(pool)
        .await
        .expect("Failed to connect to JD");

    let initiator = Initiator::from_raw_k(auth_key.into_bytes()).expect("Noise init failed");

    let (mut jd_receiver, mut jd_sender, _, _) =
        Connection::new(jd_stream, HandshakeRole::Initiator(initiator))
            .await
            .expect("Noise connection failed");

    SetupConnectionHandler::setup(&mut jd_receiver, &mut jd_sender, pool, token)
        .await
        .expect("JD setup failed");

    let token_msg = AllocateMiningJobToken {
        request_id: 1,
        user_identifier: "Health check"
            .to_string()
            .try_into()
            .expect("Infallible operation"),
    };
    let frame: SFrame =
        PoolMessages::JobDeclaration(JobDeclaration::AllocateMiningJobToken(token_msg))
            .try_into()
            .expect("Failed to convert token message to frame");

    jd_sender
        .send(frame.into())
        .await
        .expect("Failed to send allocate token msg");

    (jd_sender, jd_receiver)
}

async fn await_messages(mut msg_rx: Receiver<ExpectedMessage>) -> Result<(), String> {
    let start = Instant::now();

    // List all messages we expect to receive
    let mut expected: HashSet<ExpectedMessage> = HashSet::from_iter([
        ExpectedMessage::SetupConnectionSuccess,
        ExpectedMessage::OpenExtendedMiningChannelSuccess,
        ExpectedMessage::NewExtendedMiningJob,
        ExpectedMessage::SetNewPrevHash,
        ExpectedMessage::AllocateMiningJobTokenSuccess,
        ExpectedMessage::DeclareMiningJobSuccess,
    ]);
    loop {
        if expected.is_empty() {
            return Ok(()); // All messages have been received
        }

        tokio::select! {
            Some(msg) = msg_rx.recv() => {
                expected.remove(&msg);
            }
            _ = sleep(TIMEOUT.saturating_sub(start.elapsed())) => {
                let missing: Vec<ExpectedMessage> = expected.into_iter().collect();
                return Err(format!("Request timeout. Missing {:?} msgs", missing));
            }
            else => {
                return Err("Receiver closed before all messages received".to_string());
            }
        }
    }
}

async fn handle_pool_messages(mut receiver: Receiver<Message>, msg_tx: Sender<ExpectedMessage>) {
    while let Some(msg) = receiver.recv().await {
        let msg_type = match msg {
            PoolExtMessages::Mining(Mining::OpenExtendedMiningChannelSuccess(_)) => {
                Some(ExpectedMessage::OpenExtendedMiningChannelSuccess)
            }
            PoolExtMessages::Mining(Mining::NewExtendedMiningJob(_)) => {
                Some(ExpectedMessage::NewExtendedMiningJob)
            }
            PoolExtMessages::Mining(Mining::SetNewPrevHash(_)) => {
                Some(ExpectedMessage::SetNewPrevHash)
            }
            _ => None,
        };
        if let Some(msg_type) = msg_type {
            if msg_tx.send(msg_type).await.is_err() {
                println!("Failed to send ExpectedMessage");
                break;
            }
        }
    }
}

async fn handle_jd_messages(
    mut receiver: Receiver<Frame<PoolMessages<'static>, codec_sv2::buffer_sv2::Slice>>,
    jd_sender: Sender<Frame<PoolMessages<'static>, codec_sv2::buffer_sv2::Slice>>,
    msg_tx: Sender<ExpectedMessage>,
) {
    while let Some(message) = receiver.recv().await {
        let mut frame: StandardSv2Frame<_> = match message.try_into() {
            Ok(frame) => frame,
            Err(e) => {
                println!("Invalid upstream frame: {:?}", e);
                continue;
            }
        };
        let msg_type = frame.get_header().map(|h| h.msg_type()).unwrap_or_default();
        let payload = frame.payload();
        match (msg_type, payload).try_into() {
            Ok(JobDeclaration::AllocateMiningJobTokenSuccess(token)) => {
                if msg_tx
                    .send(ExpectedMessage::AllocateMiningJobTokenSuccess)
                    .await
                    .is_err()
                {
                    println!("Failed to send AllocateMiningJobTokenSuccess");
                    break;
                }
                let mining_job_token = B0255::from_vec_(token.mining_job_token.to_vec())
                    .expect("Failed to create owned token from Vec<u8>");
                send_declare_jobs(jd_sender.clone(), mining_job_token, token.request_id).await;
            }
            Ok(JobDeclaration::DeclareMiningJobSuccess(_)) => {
                if msg_tx
                    .send(ExpectedMessage::DeclareMiningJobSuccess)
                    .await
                    .is_err()
                {
                    println!("Failed to send DeclareMiningJobSuccess");
                    break;
                }
            }
            Ok(e) => println!("Unexpected message{:?}", e),
            Err(e) => println!("An error occured {:?}", e),
        }
    }
}

async fn send_declare_jobs(
    sender: Sender<Frame<PoolMessages<'static>, codec_sv2::buffer_sv2::Slice>>,
    token: B0255<'static>,
    request_id: u32,
) {
    println!("Checking jd messages....");
    let declare_msg = DeclareMiningJob {
        request_id,
        coinbase_prefix: B064K::from_vec_(vec![0; 48]).expect("Failed to create coinbase_prefix"),
        coinbase_suffix: B064K::from_vec_(vec![0; 128]).expect("Failed to create coinbase_suffix"),
        version: 779157504,
        mining_job_token: token,
        tx_list: Seq064K::new(vec![]).expect("Failed to create tx_list"),
        excess_data: B064K::from_vec_(vec![0; 32]).expect("Failed to create excess_data"),
    };
    let frame: SFrame = PoolMessages::JobDeclaration(JobDeclaration::DeclareMiningJob(declare_msg))
        .try_into()
        .expect("Failed to convert DeclareMiningJob into SFrame");

    if let Err(e) = sender.send(frame.into()).await {
        println!("Failed to send DeclareMiningJob: {}", e);
    }
}
