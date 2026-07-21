//! An async, streaming S3-compatible client with explicit resource bounds.

#![forbid(unsafe_code)]

pub mod client;
pub mod config;
pub mod credentials;
pub mod endpoint;
pub mod error;
pub mod operation;
pub mod retry;
pub mod stream;

mod protocol;
mod signing;
mod transport;

pub use client::S3Client;
pub use config::{AddressingStyle, S3Config, S3ConfigBuilder};
pub use credentials::{
    CachedCredentialsProvider, Credentials, CredentialsProvider, EnvironmentCredentialsProvider,
    StaticCredentialsProvider,
};
pub use endpoint::{Endpoint, EndpointUrl};
pub use error::{ErrorCategory, RetryClassification, S3Error, TimeoutPhase};
pub use operation::*;
pub use retry::RetryPolicy;
pub use stream::{ByteStream, ResponseStream};
