use std::sync::Arc;

use fuel_core_types::{
    blockchain::{
        primitives::{
            BlockHeight,
            BlockId,
        },
        SealedBlock,
        SealedBlockHeader,
    },
    fuel_tx::Transaction,
};
use libp2p::PeerId;
use serde::{
    Deserialize,
    Serialize,
};
use serde_with::{
    serde_as,
    FromInto,
};
use tokio::sync::oneshot;

pub(crate) const REQUEST_RESPONSE_PROTOCOL_ID: &[u8] = b"/fuel/req_res/0.0.1";

/// Max Size in Bytes of the Request Message
pub(crate) const MAX_REQUEST_SIZE: usize = core::mem::size_of::<RequestMessage>();

pub type ChannelItem<T> = oneshot::Sender<Option<T>>;

// Peer receives a `RequestMessage`.
// It prepares a response in form of `OutboundResponse`
// This `OutboundResponse` gets prepared to be sent over the wire in `NetworkResponse` format.
// The Peer that requested the message receives the response over the wire in `NetworkResponse` format.
// It then unpacks it into `ResponseMessage`.
// `ResponseChannelItem` is used to forward the data within `ResponseMessage` to the receving channel.
// Client Peer: `RequestMessage` (send request)
// Server Peer: `RequestMessage` (receive request) -> `OutboundResponse` -> `NetworkResponse` (send response)
// Client Peer: `NetworkResponse` (receive response) -> `ResponseMessage(data)` -> `ResponseChannelItem(channel, data)` (handle response)

#[serde_as]
#[derive(Serialize, Deserialize, Eq, PartialEq, Debug, Clone, Copy)]
pub enum RequestMessage {
    Block(BlockHeight),
    SealedHeader(BlockHeight),
    Transactions(#[serde_as(as = "FromInto<[u8; 32]>")] BlockId),
}

/// Final Response Message that p2p service sends to the Orchestrator
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ResponseMessage {
    SealedBlock(Option<SealedBlock>),
    SealedHeader(Option<SealedBlockHeader>),
    Transactions(Option<Vec<Transaction>>),
}

/// Holds oneshot channels for specific responses
#[derive(Debug)]
pub enum ResponseChannelItem {
    Block(ChannelItem<SealedBlock>),
    SealedHeader(ChannelItem<(PeerId, SealedBlockHeader)>),
    Transactions(ChannelItem<Vec<Transaction>>),
}

/// Response that is sent over the wire
/// and then additionaly deserialized into `ResponseMessage`
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum NetworkResponse {
    Block(Option<Vec<u8>>),
    Header(Option<Vec<u8>>),
    Transactions(Option<Vec<u8>>),
}

/// Initial state of the `ResponseMessage` prior to having its inner value serialized
/// and wrapped into `NetworkResponse`
#[derive(Debug, Clone)]
pub enum OutboundResponse {
    Block(Option<Arc<SealedBlock>>),
    SealedHeader(Option<Arc<SealedBlockHeader>>),
    Transactions(Option<Arc<Vec<Transaction>>>),
}

#[derive(Debug)]
pub enum RequestError {
    NoPeersConnected,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ResponseError {
    ResponseChannelDoesNotExist,
    SendingResponseFailed,
    ConversionToIntermediateFailed,
}
