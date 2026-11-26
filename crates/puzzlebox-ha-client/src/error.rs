use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not connected to Home Assistant")]
    NotConnected,
    #[error("connection lost before a reply arrived")]
    Disconnected,
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("Home Assistant returned an error ({code}): {message}")]
    Ha {
        code: String,
        message: String,
        /// Names the kind of error, for those HA has a translated message for.
        translation_key: Option<String>,
    },
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("websocket: {0}")]
    WebSocket(Box<tokio_tungstenite::tungstenite::Error>),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("client task has shut down")]
    Closed,
}

impl Error {
    pub(crate) fn from_ha_error(err: Option<&Value>) -> Self {
        let field =
            |k: &str| err.and_then(|e| e.get(k)).and_then(Value::as_str).unwrap_or("unknown").to_owned();
        let translation_key =
            err.and_then(|e| e.get("translation_key")).and_then(Value::as_str).map(str::to_owned);
        Error::Ha { code: field("code"), message: field("message"), translation_key }
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::WebSocket(Box::new(e))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
