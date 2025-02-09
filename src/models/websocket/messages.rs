use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct WebSocketMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<usize>,
    #[serde(flatten)]
    pub r#type: WebSocketMessageInner,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum WebSocketMessageInner {
    Hello {
        #[serde(flatten)]
        motd: serde_json::Value,
    },

    Keepalive {
        server_time: String,
    },

    Response {
        responding_to: String,
        #[serde(flatten)]
        data: WebSocketMessageResponse,
    },

    Work,

    MakeTransaction {
        /// The privatekey of your address.
        #[serde(rename = "privatekey")]
        private_key: String,

        /// The recipient of the transaction.
        to: String,

        /// The amount to send to the recipient.
        amount: u32,

        /// Optional metadata to include in the transaction.
        metadata: Option<String>,
    },

    GetValidSubscriptionLevels,

    Address {
        address: String,

        /// When supplied, fetch the count of names owned by the address.
        #[serde(rename = "fetchNames")]
        fetch_names: Option<bool>,
    },

    Me,
    GetSubscriptionLevel,
    Logout,
    Login {
        #[serde(rename = "privatekey")]
        private_key: String,
    },

    Subscribe {
        event: String,
    },

    Unsubscribe {
        event: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "responding_to", rename_all = "snake_case")]
pub enum WebSocketMessageResponse {
    Work {
        /// The current Krist work (difficulty)
        work: usize,
    },

    MakeTransaction {
        transaction: serde_json::Value,
    },

    GetValidSubscriptionLevels {
        /// All valid subscription levels
        valid_subscription_levels: serde_json::Value,
    },

    Address {
        address: serde_json::Value,
    },

    Me {
        /// Whether the current user is a guest or not
        is_guest: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        address: Option<serde_json::Value>,
    },

    GetSubscriptionLevel {
        subscription_level: Vec<String>,
    },

    Logout {
        /// Whether the current user is a guest or not
        is_guest: bool,
    },

    Login {
        /// Whether the current user is a guest or not
        is_guest: bool,
        address: serde_json::Value,
    },

    Subscribe {
        subscription_level: Vec<String>,
    },

    Unsubscribe {
        subscription_level: Vec<String>,
    },
}
