//! Server error type and its HTTP representation.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Everything that can go wrong servicing a request.
#[derive(Debug)]
pub enum ServerError {
    /// No device with this id is currently present.
    UnknownDevice(String),
    /// The device is present but its model has no known command registry.
    NoRegistry(String),
    /// The command is not in this device's registry.
    UnknownCommand { device: String, command: String },
    /// A field value was the wrong shape or out of range.
    BadValue(String),
    /// The device did not answer within the request timeout.
    Timeout { device: String, command: String },
    /// The device answered the command with a refusal (the reply id with its top bit set).
    Refused { device: String, command: String },
    /// The device worker is gone (thread died or shut down).
    DeviceGone(String),
    /// Protocol-level failure building or parsing bytes.
    Protocol(String),
    /// Workspace persistence failure.
    Storage(String),
    /// A backend was requested that is not implemented yet.
    Unsupported(String),
}

impl ServerError {
    /// Stable machine-readable code, for clients that switch on it.
    pub fn code(&self) -> &'static str {
        match self {
            ServerError::UnknownDevice(_) => "unknown_device",
            ServerError::NoRegistry(_) => "no_registry",
            ServerError::UnknownCommand { .. } => "unknown_command",
            ServerError::BadValue(_) => "bad_value",
            ServerError::Timeout { .. } => "timeout",
            ServerError::Refused { .. } => "refused",
            ServerError::DeviceGone(_) => "device_gone",
            ServerError::Protocol(_) => "protocol_error",
            ServerError::Storage(_) => "storage_error",
            ServerError::Unsupported(_) => "unsupported",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            ServerError::UnknownDevice(_) | ServerError::UnknownCommand { .. } => {
                StatusCode::NOT_FOUND
            }
            ServerError::BadValue(_) => StatusCode::BAD_REQUEST,
            ServerError::Timeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            // The device answered, and its answer was no: a gateway's upstream failing, as a
            // timeout is, but not one worth waiting on.
            ServerError::Refused { .. } => StatusCode::BAD_GATEWAY,
            ServerError::DeviceGone(_) => StatusCode::SERVICE_UNAVAILABLE,
            ServerError::Protocol(_) | ServerError::Storage(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            // A device whose model we do not recognise, and backends not yet built,
            // are both "understood the request, cannot do it" rather than errors in
            // the request itself.
            ServerError::NoRegistry(_) | ServerError::Unsupported(_) => {
                StatusCode::NOT_IMPLEMENTED
            }
        }
    }
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::UnknownDevice(d) => write!(f, "no such device: {d}"),
            ServerError::NoRegistry(d) => {
                write!(f, "device {d} has no known command registry for its model")
            }
            ServerError::UnknownCommand { device, command } => {
                write!(f, "device {device} has no command '{command}'")
            }
            ServerError::BadValue(m) => write!(f, "bad value: {m}"),
            ServerError::Timeout { device, command } => {
                write!(f, "device {device} timed out running '{command}'")
            }
            ServerError::Refused { device, command } => {
                write!(f, "device {device} refused '{command}'")
            }
            ServerError::DeviceGone(d) => write!(f, "device {d} is no longer reachable"),
            ServerError::Protocol(m) => write!(f, "protocol error: {m}"),
            ServerError::Storage(m) => write!(f, "storage error: {m}"),
            ServerError::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for ServerError {}

impl ServerError {
    /// The JSON body this error serialises to, also used for WS `rpc_error` frames.
    pub fn to_json(&self) -> serde_json::Value {
        json!({"error": {"code": self.code(), "message": self.to_string()}})
    }
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        (self.status(), axum::Json(self.to_json())).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_has_its_own_code_status_and_message() {
        let e = ServerError::Refused { device: "usb:1".into(), command: "get_feature_mask".into() };
        assert_eq!(e.code(), "refused");
        assert_eq!(e.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(e.to_string(), "device usb:1 refused 'get_feature_mask'");
        // The HTTP body and the WS `rpc_error` frame both carry this.
        assert_eq!(e.to_json(), json!({"error": {"code": "refused", "message": "device usb:1 refused 'get_feature_mask'"}}));
    }
}
