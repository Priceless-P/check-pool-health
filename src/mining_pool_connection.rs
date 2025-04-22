use codec_sv2::{buffer_sv2::Slice, HandshakeRole};
use demand_share_accounting_ext::parser::PoolExtMessages;
use demand_sv2_connection::noise_connection_tokio::Connection;
use key_utils::Secp256k1PublicKey;
use noise_sv2::Initiator;
use rand::distributions::DistString;
use roles_logic_sv2::{
    common_messages_sv2::SetupConnection, mining_sv2::OpenExtendedMiningChannel, parsers::Mining,
};
use tokio::{
    net::TcpStream,
    sync::mpsc::{Receiver, Sender},
};

use crate::{errors, utils::AbortOnDrop, EitherFrame, StdFrame};

pub async fn mining_setup_connection(
    recv: &mut Receiver<EitherFrame>,
    send: &mut Sender<EitherFrame>,
    setup_conection: SetupConnection<'static>,
    timer: std::time::Duration,
) -> Result<roles_logic_sv2::common_messages_sv2::SetupConnectionSuccess, errors::Error> {
    let msg = PoolExtMessages::Common(roles_logic_sv2::parsers::CommonMessages::SetupConnection(
        setup_conection,
    ));
    let std_frame: StdFrame = match msg.try_into() {
        Ok(frame) => frame,
        Err(e) => {
            return Err(errors::Error::RolesSv2Logic(e));
        }
    };
    let either_frame: EitherFrame = std_frame.into();
    if send.send(either_frame).await.is_err() {
        return Err(errors::Error::Unrecoverable);
    }

    if let Ok(Some(msg)) = tokio::time::timeout(timer, recv.recv()).await {
        let mut msg: StdFrame = msg.try_into().map_err(errors::Error::FramingSv2)?;
        let header = msg.get_header().ok_or(errors::Error::UnexpectedMessage)?;
        let message_type = header.msg_type();
        let payload = msg.payload();
        let msg: roles_logic_sv2::parsers::CommonMessages<'_> =
            match (message_type, payload).try_into() {
                Ok(message) => message,
                Err(e) => {
                    return Err(errors::Error::UpstreamIncoming(e));
                }
            };
        match msg {
            roles_logic_sv2::parsers::CommonMessages::SetupConnectionSuccess(s) => Ok(s),
            _ => Err(errors::Error::UnexpectedMessage),
        }
    } else {
        Err(errors::Error::Timeout)
    }
}

pub fn get_mining_setup_connection_msg(work_selection: bool) -> SetupConnection<'static> {
    let endpoint_host = "0.0.0.0"
        .to_string()
        .into_bytes()
        .try_into()
        .expect("Internal error: conversion failed");
    let vendor = String::new()
        .try_into()
        .expect("Internal error: conversion failed for vendor");
    let hardware_version = String::new()
        .try_into()
        .expect("Internal error: conversion failed for hardware_version");
    let firmware = String::new()
        .try_into()
        .expect("Internal error: conversion failed for firmware");
    let flags = if work_selection {
        0b0000_0000_0000_0000_0000_0000_0000_0110
    } else {
        0b0000_0000_0000_0000_0000_0000_0000_0100
    };
    let token = std::env::var("TOKEN").expect("TOKEN environment variable not set");
    let device_id = rand::distributions::Alphanumeric.sample_string(&mut rand::thread_rng(), 16);
    let device_id = format!("{}::POOLED::{}", device_id, token)
        .try_into()
        .expect("Internal error: conversion failed for device_id");
    SetupConnection {
        protocol: roles_logic_sv2::common_messages_sv2::Protocol::MiningProtocol,
        min_version: 2,
        max_version: 2,
        flags,
        endpoint_host,
        endpoint_port: 50,
        vendor,
        hardware_version,
        firmware,
        device_id,
    }
}

pub fn relay_up(
    mut recv: Receiver<PoolExtMessages<'static>>,
    send: Sender<EitherFrame>,
) -> AbortOnDrop {
    let task = tokio::spawn(async move {
        while let Some(msg) = recv.recv().await {
            let std_frame: Result<StdFrame, _> = msg.try_into();
            if let Ok(std_frame) = std_frame {
                let either_frame: EitherFrame = std_frame.into();
                if send.send(either_frame).await.is_err() {
                    eprintln!("Mining upstream failed");
                    break;
                };
            } else {
                panic!("Internal Mining downstream try to send invalid message");
            }
        }
    });
    task.into()
}

pub fn relay_down(
    mut recv: Receiver<EitherFrame>,
    send: Sender<PoolExtMessages<'static>>,
) -> AbortOnDrop {
    let task = tokio::spawn(async move {
        while let Some(msg) = recv.recv().await {
            let msg: Result<StdFrame, ()> = msg.try_into().map_err(|_| ());
            if let Ok(mut msg) = msg {
                if let Some(header) = msg.get_header() {
                    let message_type = header.msg_type();
                    let payload = msg.payload();
                    let extension = header.ext_type();
                    let msg: Result<PoolExtMessages<'_>, _> =
                        (extension, message_type, payload).try_into();
                    if let Ok(msg) = msg {
                        let msg = msg.into_static();
                        if send.send(msg).await.is_err() {
                            eprintln!("Internal Mining downstream not available");
                        }
                    } else {
                        eprintln!("Mining Upstream send non Mining message. Disconnecting");
                        break;
                    }
                } else {
                    eprintln!("Mining Upstream send invalid message no header. Disconnecting");
                    break;
                }
            } else {
                eprintln!("Mining Upstream down.");
                break;
            }
        }
        eprintln!("Failed to receive msg from Pool");
    });
    task.into()
}

pub fn open_channel() -> Mining<'static> {
    roles_logic_sv2::parsers::Mining::OpenExtendedMiningChannel(OpenExtendedMiningChannel {
        request_id: 0,
        max_target: binary_sv2::u256_from_int(u64::MAX),
        min_extranonce_size: 8,
        user_identity: "health-check"
            .to_string()
            .try_into()
            .expect("Failed to convert user identity"),
        nominal_hash_rate: 0.0,
    })
}

pub async fn initialize_mining_connections(
    setup_connection_msg: Option<SetupConnection<'static>>,
    stream: TcpStream,
    authority_public_key: Secp256k1PublicKey,
) -> Result<
    (
        Receiver<codec_sv2::Frame<PoolExtMessages<'static>, Slice>>,
        Sender<codec_sv2::Frame<PoolExtMessages<'static>, Slice>>,
        SetupConnection<'static>,
    ),
    (),
> {
    let initiator =
        // Safe expect: Key is a constant and must be right.
        Initiator::from_raw_k(authority_public_key.into_bytes()).expect("Invalid authority key");
    let (receiver, sender, _, _) =
        match Connection::new(stream, HandshakeRole::Initiator(initiator)).await {
            Ok(connection) => connection,
            Err(_) => {
                return Err(());
            }
        };
    let setup_connection_msg =
        setup_connection_msg.unwrap_or_else(|| get_mining_setup_connection_msg(true));
    Ok((receiver, sender, setup_connection_msg))
}
