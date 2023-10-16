use serde::{Deserialize, Serialize};

use crate::{
    error::ErrorDetails,
    get_receive_address::GetReceiveAddressParams,
    register_multisig::RegisterMultisigParams,
    sign_liquid_tx::{SignLiquidTxParams, TxInputParams},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Request<P> {
    pub id: String,
    pub method: String,
    pub params: Option<P>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Params {
    Epoch(EpochParams),
    Entropy(EntropyParams),
    AuthUser(AuthUserParams),
    Handshake(HandshakeParams),
    UpdatePinServer(UpdatePinserverParams),
    HandshakeComplete(HandshakeCompleteParams),
    GetXpub(GetXpubParams),
    GetReceiveAddress(GetReceiveAddressParams),
    SignMessage(SignMessageParams),
    GetSignature(GetSignatureParams),
    SignLiquidTx(SignLiquidTxParams),
    DebugSetMnemonic(DebugSetMnemonicParams),
    TxInput(TxInputParams),
    RegisterMultisig(RegisterMultisigParams),
}

#[derive(Debug, Serialize)]
pub struct AuthUserParams {
    pub network: crate::Network,
    pub epoch: u64,
}

#[derive(Debug, Serialize)]
pub struct EpochParams {
    pub epoch: u64,
}

#[derive(Debug, Serialize)]
pub struct EntropyParams {
    #[serde(with = "serde_bytes")]
    pub entropy: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HandshakeParams {
    pub sig: String,
    pub ske: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HandshakeCompleteParams {
    pub encrypted_key: String,
    pub hmac: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetXpubParams {
    pub network: crate::Network,

    /// Derive the master node (m) with the given path and the return the resuting xpub
    pub path: Vec<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SignMessageParams {
    pub message: String,
    pub path: Vec<u32>,

    #[serde(with = "serde_bytes")]
    pub ae_host_commitment: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetSignatureParams {
    /// 32 bytes anti-exfiltration entropy
    #[serde(with = "serde_bytes")]
    pub ae_host_entropy: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HandshakeComplete {
    pub encrypted_data: String,
    pub hmac_encrypted_data: String,
    pub ske: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DebugSetMnemonicParams {
    pub mnemonic: String,
    pub passphrase: Option<String>,
    pub temporary_wallet: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Response<T> {
    pub id: String,
    pub result: Option<T>,
    pub error: Option<ErrorDetails>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub struct VersionInfoResult {
    pub jade_version: String,
    pub jade_ota_max_chunk: u32,
    pub jade_config: String,
    pub board_type: String,
    pub jade_features: String,
    pub idf_version: String,
    pub chip_features: String,
    pub efusemac: String,
    pub battery_status: u8,
    pub jade_state: JadeState,
    pub jade_networks: String,
    pub jade_has_pin: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]

pub enum JadeState {
    /// no wallet set on the hw, mnemonic not entered, unit uninitialised
    Uninit,

    /// wallet mnemonic has been set on hw, but not yet persisted with blind pinserver
    Unsaved,

    /// wallet set, but currently locked - requires PIN entry to unlock.
    Locked,

    /// wallet set and unlocked for this interface, ready to use.
    Ready,

    ///  hw currently set with a temporary ('Emergency Restore') wallet, ready to use.
    Temp,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RegisteredMultisig {
    variant: String,
    sorted: bool,
    threshold: u32,
    num_signers: u32,

    #[serde(with = "serde_bytes")]
    master_blinding_key: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AuthResult<T> {
    http_request: HttpRequest<T>,
}

impl<T> AuthResult<T> {
    pub fn urls(&self) -> &[String] {
        self.http_request.params.urls.as_slice()
    }
    pub fn data(&self) -> &T {
        &self.http_request.params.data
    }
}
#[derive(Debug, Deserialize, Serialize)]
pub struct HttpRequest<T> {
    params: HttpParams<T>,
    #[serde(rename = "on-reply")]
    on_reply: String,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct HttpParams<T> {
    urls: Vec<String>,
    method: String,
    accept: String,
    data: T,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HandshakeData {
    cke: String,
    encrypted_data: String,
    hmac_encrypted_data: String,
    ske: String,
    error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdatePinserverParams {
    pub reset_details: bool,
    pub reset_certificate: bool,

    #[serde(rename = "urlA")]
    pub url_a: String,

    #[serde(rename = "urlB")]
    pub url_b: String,

    #[serde(with = "serde_bytes")]
    pub pubkey: Vec<u8>,
    pub certificate: String,
}
