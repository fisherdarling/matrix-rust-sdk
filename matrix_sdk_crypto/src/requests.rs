// Copyright 2020 The Matrix.org Foundation C.I.C.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use matrix_sdk_common::uuid::Uuid;
use ruma::{
    api::client::r0::{
        keys::{
            claim_keys::Response as KeysClaimResponse,
            get_keys::Response as KeysQueryResponse,
            upload_keys::{Request as KeysUploadRequest, Response as KeysUploadResponse},
            upload_signatures::{
                Request as SignatureUploadRequest, Response as SignatureUploadResponse,
            },
            upload_signing_keys::Response as SigningKeysUploadResponse,
            CrossSigningKey,
        },
        message::send_message_event::Response as RoomMessageResponse,
        to_device::{send_event_to_device::Response as ToDeviceResponse, DeviceIdOrAllDevices},
    },
    events::{AnyMessageEventContent, AnyToDeviceEventContent, EventContent, EventType},
    DeviceIdBox, RoomId, UserId,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue as RawJsonValue;

/// Customized version of
/// `ruma_client_api::r0::to_device::send_event_to_device::Request`,
/// using a UUID for the transaction ID.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToDeviceRequest {
    /// Type of event being sent to each device.
    pub event_type: EventType,

    /// A request identifier unique to the access token used to send the
    /// request.
    pub txn_id: Uuid,

    /// A map of users to devices to a content for a message event to be
    /// sent to the user's device. Individual message events can be sent
    /// to devices, but all events must be of the same type.
    /// The content's type for this field will be updated in a future
    /// release, until then you can create a value using
    /// `serde_json::value::to_raw_value`.
    pub messages: BTreeMap<UserId, BTreeMap<DeviceIdOrAllDevices, Box<RawJsonValue>>>,
}

impl ToDeviceRequest {
    /// Create a new owned to-device request
    ///
    /// # Arguments
    ///
    /// * `recipient` - The ID of the user that should receive this to-device
    /// event.
    ///
    /// * `recipient_device` - The device that should receive this to-device
    /// event, or all devices.
    ///
    /// * `content` - The content of the to-device event.
    pub(crate) fn new(
        recipient: &UserId,
        recipient_device: impl Into<DeviceIdOrAllDevices>,
        content: AnyToDeviceEventContent,
    ) -> Self {
        let mut messages = BTreeMap::new();
        let mut user_messages = BTreeMap::new();

        user_messages.insert(
            recipient_device.into(),
            serde_json::value::to_raw_value(&content).expect("Can't serialize to-device content"),
        );
        messages.insert(recipient.clone(), user_messages);
        let event_type = EventType::from(content.event_type());

        ToDeviceRequest { txn_id: Uuid::new_v4(), event_type, messages }
    }

    /// Gets the transaction ID as a string.
    pub fn txn_id_string(&self) -> String {
        self.txn_id.to_string()
    }

    /// Get the number of unique messages this request contains.
    ///
    /// *Note*: A single message may be sent to multiple devices, so this may or
    /// may not be the number of devices that will receive the messages as well.
    pub fn message_count(&self) -> usize {
        self.messages.values().map(|d| d.len()).sum()
    }
}

/// Request that will publish a cross signing identity.
///
/// This uploads the public cross signing key triplet.
#[derive(Debug, Clone)]
pub struct UploadSigningKeysRequest {
    /// The user's master key.
    pub master_key: Option<CrossSigningKey>,
    /// The user's self-signing key. Must be signed with the accompanied master,
    /// or by the user's most recently uploaded master key if no master key
    /// is included in the request.
    pub self_signing_key: Option<CrossSigningKey>,
    /// The user's user-signing key. Must be signed with the accompanied master,
    /// or by the user's most recently uploaded master key if no master key
    /// is included in the request.
    pub user_signing_key: Option<CrossSigningKey>,
}

/// Customized version of
/// `ruma_client_api::r0::keys::get_keys::Request`, without any
/// references.
#[derive(Clone, Debug)]
pub struct KeysQueryRequest {
    /// The time (in milliseconds) to wait when downloading keys from remote
    /// servers. 10 seconds is the recommended default.
    pub timeout: Option<Duration>,

    /// The keys to be downloaded. An empty list indicates all devices for
    /// the corresponding user.
    pub device_keys: BTreeMap<UserId, Vec<DeviceIdBox>>,

    /// If the client is fetching keys as a result of a device update
    /// received in a sync request, this should be the 'since' token of that
    /// sync request, or any later sync token. This allows the server to
    /// ensure its response contains the keys advertised by the notification
    /// in that sync.
    pub token: Option<String>,
}

impl KeysQueryRequest {
    pub(crate) fn new(device_keys: BTreeMap<UserId, Vec<DeviceIdBox>>) -> Self {
        Self { timeout: None, device_keys, token: None }
    }
}

/// Enum over the different outgoing requests we can have.
#[derive(Debug)]
pub enum OutgoingRequests {
    /// The keys upload request, uploading device and one-time keys.
    KeysUpload(KeysUploadRequest),
    /// The keys query request, fetching the device and cross singing keys of
    /// other users.
    KeysQuery(KeysQueryRequest),
    /// The to-device requests, this request is used for a couple of different
    /// things, the main use is key requests/forwards and interactive device
    /// verification.
    ToDeviceRequest(ToDeviceRequest),
    /// Signature upload request, this request is used after a successful device
    /// or user verification is done.
    SignatureUpload(SignatureUploadRequest),
    /// A room message request, usually for sending in-room interactive
    /// verification events.
    RoomMessage(RoomMessageRequest),
}

#[cfg(test)]
impl OutgoingRequests {
    pub fn to_device(&self) -> Option<&ToDeviceRequest> {
        match self {
            OutgoingRequests::ToDeviceRequest(r) => Some(r),
            _ => None,
        }
    }
}

impl From<KeysQueryRequest> for OutgoingRequests {
    fn from(request: KeysQueryRequest) -> Self {
        OutgoingRequests::KeysQuery(request)
    }
}

impl From<KeysUploadRequest> for OutgoingRequests {
    fn from(request: KeysUploadRequest) -> Self {
        OutgoingRequests::KeysUpload(request)
    }
}

impl From<ToDeviceRequest> for OutgoingRequests {
    fn from(request: ToDeviceRequest) -> Self {
        OutgoingRequests::ToDeviceRequest(request)
    }
}

impl From<RoomMessageRequest> for OutgoingRequests {
    fn from(request: RoomMessageRequest) -> Self {
        OutgoingRequests::RoomMessage(request)
    }
}

impl From<SignatureUploadRequest> for OutgoingRequests {
    fn from(request: SignatureUploadRequest) -> Self {
        OutgoingRequests::SignatureUpload(request)
    }
}

impl From<OutgoingVerificationRequest> for OutgoingRequest {
    fn from(r: OutgoingVerificationRequest) -> Self {
        Self { request_id: r.request_id(), request: Arc::new(r.into()) }
    }
}

impl From<SignatureUploadRequest> for OutgoingRequest {
    fn from(r: SignatureUploadRequest) -> Self {
        Self { request_id: Uuid::new_v4(), request: Arc::new(r.into()) }
    }
}

/// Enum over all the incoming responses we need to receive.
#[derive(Debug)]
pub enum IncomingResponse<'a> {
    /// The keys upload response, notifying us about the amount of uploaded
    /// one-time keys.
    KeysUpload(&'a KeysUploadResponse),
    /// The keys query response, giving us the device and cross singing keys of
    /// other users.
    KeysQuery(&'a KeysQueryResponse),
    /// The to-device response, an empty response.
    ToDevice(&'a ToDeviceResponse),
    /// The key claiming requests, giving us new one-time keys of other users so
    /// new Olm sessions can be created.
    KeysClaim(&'a KeysClaimResponse),
    /// The cross signing keys upload response, marking our private cross
    /// signing identity as shared.
    SigningKeysUpload(&'a SigningKeysUploadResponse),
    /// The cross signing signature upload response.
    SignatureUpload(&'a SignatureUploadResponse),
    /// A room message response, usually for interactive verifications.
    RoomMessage(&'a RoomMessageResponse),
}

impl<'a> From<&'a KeysUploadResponse> for IncomingResponse<'a> {
    fn from(response: &'a KeysUploadResponse) -> Self {
        IncomingResponse::KeysUpload(response)
    }
}

impl<'a> From<&'a KeysQueryResponse> for IncomingResponse<'a> {
    fn from(response: &'a KeysQueryResponse) -> Self {
        IncomingResponse::KeysQuery(response)
    }
}

impl<'a> From<&'a ToDeviceResponse> for IncomingResponse<'a> {
    fn from(response: &'a ToDeviceResponse) -> Self {
        IncomingResponse::ToDevice(response)
    }
}

impl<'a> From<&'a RoomMessageResponse> for IncomingResponse<'a> {
    fn from(response: &'a RoomMessageResponse) -> Self {
        IncomingResponse::RoomMessage(response)
    }
}

impl<'a> From<&'a KeysClaimResponse> for IncomingResponse<'a> {
    fn from(response: &'a KeysClaimResponse) -> Self {
        IncomingResponse::KeysClaim(response)
    }
}

impl<'a> From<&'a SignatureUploadResponse> for IncomingResponse<'a> {
    fn from(response: &'a SignatureUploadResponse) -> Self {
        IncomingResponse::SignatureUpload(response)
    }
}

/// Outgoing request type, holds the unique ID of the request and the actual
/// request.
#[derive(Debug, Clone)]
pub struct OutgoingRequest {
    /// The unique id of a request, needs to be passed when receiving a
    /// response.
    pub(crate) request_id: Uuid,
    /// The underlying outgoing request.
    pub(crate) request: Arc<OutgoingRequests>,
}

impl OutgoingRequest {
    /// Get the unique id of this request.
    pub fn request_id(&self) -> &Uuid {
        &self.request_id
    }

    /// Get the underlying outgoing request.
    pub fn request(&self) -> &OutgoingRequests {
        &self.request
    }
}

/// Customized owned request type for sending out room messages.
#[derive(Clone, Debug)]
pub struct RoomMessageRequest {
    /// The room to send the event to.
    pub room_id: RoomId,

    /// The transaction ID for this event.
    ///
    /// Clients should generate an ID unique across requests with the
    /// same access token; it will be used by the server to ensure
    /// idempotency of requests.
    pub txn_id: Uuid,

    /// The event content to send.
    pub content: AnyMessageEventContent,
}

/// An enum over the different outgoing verification based requests.
#[derive(Clone, Debug)]
pub enum OutgoingVerificationRequest {
    /// The to-device verification request variant.
    ToDevice(ToDeviceRequest),
    /// The in-room verification request variant.
    InRoom(RoomMessageRequest),
}

impl OutgoingVerificationRequest {
    /// Get the unique id of this request.
    pub fn request_id(&self) -> Uuid {
        match self {
            OutgoingVerificationRequest::ToDevice(t) => t.txn_id,
            OutgoingVerificationRequest::InRoom(r) => r.txn_id,
        }
    }
}

impl From<ToDeviceRequest> for OutgoingVerificationRequest {
    fn from(r: ToDeviceRequest) -> Self {
        OutgoingVerificationRequest::ToDevice(r)
    }
}

impl From<RoomMessageRequest> for OutgoingVerificationRequest {
    fn from(r: RoomMessageRequest) -> Self {
        OutgoingVerificationRequest::InRoom(r)
    }
}

impl From<OutgoingVerificationRequest> for OutgoingRequests {
    fn from(request: OutgoingVerificationRequest) -> Self {
        match request {
            OutgoingVerificationRequest::ToDevice(r) => OutgoingRequests::ToDeviceRequest(r),
            OutgoingVerificationRequest::InRoom(r) => OutgoingRequests::RoomMessage(r),
        }
    }
}
