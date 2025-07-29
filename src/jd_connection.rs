use codec_sv2::{StandardEitherFrame, StandardSv2Frame};
use rand::distributions::{Alphanumeric, DistString};
use roles_logic_sv2::{
    common_messages_sv2::{Protocol, SetupConnection},
    handlers::{common::ParseUpstreamCommonMessages, SendTo_ as CSendTo},
    parsers::PoolMessages,
    routing_logic::{CommonRoutingLogic, NoRouting},
    utils::Mutex,
};
use std::{convert::TryInto, net::SocketAddr, sync::Arc};
use tokio::sync::mpsc::{Receiver as TReceiver, Sender as TSender};

pub type Message = PoolMessages<'static>;
pub type StdFrame = StandardSv2Frame<Message>;

use crate::errors::Error;
pub type EitherFrame = StandardEitherFrame<Message>;
pub struct SetupConnectionHandler {}

impl SetupConnectionHandler {
    fn get_setup_connection_message(
        proxy_address: SocketAddr,
        token: String,
    ) -> SetupConnection<'static> {
        let endpoint_host = proxy_address
            .ip()
            .to_string()
            .into_bytes()
            .try_into()
            .expect("Internal error: this operation can not fail because IP addr string can always be converted into Inner");
        let vendor = String::new().try_into().expect("Internal error: this operation can not fail because empty string can always be converted into Inner");
        let hardware_version = String::new().try_into().expect("Internal error: this operation can not fail because empty string can always be converted into Inner");
        let firmware = String::new().try_into().expect("Internal error: this operation can not fail because empty string can always be converted into Inner");
        let device_id = Alphanumeric.sample_string(&mut rand::thread_rng(), 16);
        let device_id = format!("{}::POOLED::{}", device_id, token)
            .to_string()
            .try_into()
            .expect("Internal error: this operation can not fail because device_id string can always be converted into Inner");
        let mut setup_connection = SetupConnection {
            protocol: Protocol::JobDeclarationProtocol,
            min_version: 2,
            max_version: 2,
            flags: 0b0000_0000_0000_0000_0000_0000_0000_0000,
            endpoint_host,
            endpoint_port: proxy_address.port(),
            vendor,
            hardware_version,
            firmware,
            device_id,
        };
        setup_connection.set_async_job_nogotiation();
        setup_connection
    }

    pub async fn setup(
        receiver: &mut TReceiver<EitherFrame>,
        sender: &mut TSender<EitherFrame>,
        proxy_address: SocketAddr,
        token: String,
    ) -> Result<(), Error> {
        let setup_connection = Self::get_setup_connection_message(proxy_address, token);

        let sv2_frame: StdFrame = PoolMessages::Common(setup_connection.into()).try_into()?;
        let sv2_frame = sv2_frame.into();

        sender
            .send(sv2_frame)
            .await
            .map_err(|_| Error::Unrecoverable)?;

        let mut incoming: StdFrame = match receiver.recv().await {
            Some(msg) => msg.try_into()?,
            None => {
                eprintln!("Failed to receive msg from pool");
                return Err(Error::Unrecoverable); // Better Error to return?
            }
        };

        let message_type = incoming
            .get_header()
            .ok_or(Error::Unrecoverable)?
            .msg_type();
        let payload = incoming.payload();
        ParseUpstreamCommonMessages::handle_message_common(
            Arc::new(Mutex::new(SetupConnectionHandler {})),
            message_type,
            payload,
            CommonRoutingLogic::None,
        )?;
        Ok(())
    }
}

impl ParseUpstreamCommonMessages<NoRouting> for SetupConnectionHandler {
    fn handle_setup_connection_success(
        &mut self,
        _: roles_logic_sv2::common_messages_sv2::SetupConnectionSuccess,
    ) -> Result<roles_logic_sv2::handlers::common::SendTo, roles_logic_sv2::errors::Error> {
        Ok(CSendTo::None(None))
    }

    fn handle_setup_connection_error(
        &mut self,
        _: roles_logic_sv2::common_messages_sv2::SetupConnectionError,
    ) -> Result<roles_logic_sv2::handlers::common::SendTo, roles_logic_sv2::errors::Error> {
        todo!()
    }

    fn handle_channel_endpoint_changed(
        &mut self,
        _: roles_logic_sv2::common_messages_sv2::ChannelEndpointChanged,
    ) -> Result<roles_logic_sv2::handlers::common::SendTo, roles_logic_sv2::errors::Error> {
        todo!()
    }
}
