// apcore-cli — Security module re-exports.
// Protocol spec: SEC-01 through SEC-04

pub mod audit;
pub mod auth;
pub mod config_encryptor;
pub mod sandbox;

pub use audit::AuditLogger;
pub use auth::{AuthProvider, AuthenticationError};
pub use config_encryptor::{ConfigDecryptionError, ConfigEncryptor};
pub use sandbox::{ModuleExecutionError, ModuleNotFoundError, Sandbox, SchemaValidationError};
