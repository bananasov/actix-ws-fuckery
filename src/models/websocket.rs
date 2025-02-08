use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct WebSocketTokenData {
    pub address: String,
    pub private_key: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct WebSocketStartConnectionBody {
    #[serde(rename = "privatekey")]
    pub private_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct WebSocketStartResponse {
    pub ok: bool,
    pub url: String,
    pub expires: u16,
}

impl WebSocketTokenData {
    #[inline]
    pub fn new(address: String, private_key: Option<String>) -> Self {
        Self {
            address,
            private_key,
        }
    }
}
