use dashmap::DashSet;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct WebSocketTokenData {
    pub address: String,
    pub private_key: Option<String>,
}

#[derive(Clone)]
pub struct WebSocketSessionData {
    pub address: String,
    pub private_key: Option<String>,
    pub session: actix_ws::Session,
    pub subscriptions: DashSet<WebSocketSubscriptionType>,
}

#[derive(Clone, Debug, Hash, Eq, Serialize, Deserialize, PartialEq, PartialOrd)]
#[serde(rename_all = "camelCase")]
pub enum WebSocketSubscriptionType {
    Blocks,
    OwnBlocks,
    Transactions,
    OwnTransactions,
    Names,
    OwnNames,
    Motd,
}

impl std::str::FromStr for WebSocketSubscriptionType {
    type Err = ();

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "blocks" => Ok(Self::Blocks),
            "ownBlocks" => Ok(Self::OwnBlocks),
            "transactions" => Ok(Self::Transactions),
            "ownTransactions" => Ok(Self::OwnTransactions),
            "names" => Ok(Self::Names),
            "ownNames" => Ok(Self::OwnNames),
            "motd" => Ok(Self::Motd),
            _ => Err(()),
        }
    }
}

impl std::fmt::Display for WebSocketSubscriptionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocks => write!(f, "blocks"),
            Self::OwnBlocks => write!(f, "ownBlocks"),
            Self::Transactions => write!(f, "transactions"),
            Self::OwnTransactions => write!(f, "ownTransactions"),
            Self::Names => write!(f, "names"),
            Self::OwnNames => write!(f, "ownNames"),
            Self::Motd => write!(f, "motd"),
        }
    }
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
