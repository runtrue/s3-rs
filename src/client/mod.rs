//! S3 client implementation.

mod multipart;
mod multipart_upload;
mod object;
mod presign;
mod request;

use std::fmt;
use std::sync::Arc;

use crate::config::S3Config;
use crate::error::S3Error;
use crate::transport::Transport;

struct ClientInner {
    config: S3Config,
    transport: Transport,
}

/// An async S3-compatible client.
#[derive(Clone)]
pub struct S3Client {
    inner: Arc<ClientInner>,
}

impl S3Client {
    /// Constructs a client from validated configuration.
    pub fn new(config: S3Config) -> Result<Self, S3Error> {
        let transport = Transport::new(
            config.connect_timeout(),
            config.user_agent(),
            !config.endpoint().is_https(),
        )?;
        Ok(Self {
            inner: Arc::new(ClientInner { config, transport }),
        })
    }

    /// Returns this client's validated configuration.
    pub fn config(&self) -> &S3Config {
        &self.inner.config
    }

    fn operation_target(&self, key: Option<&str>) -> Result<crate::endpoint::EndpointUrl, S3Error> {
        self.inner.config.endpoint().object_url(
            self.inner.config.bucket(),
            key,
            self.inner.config.addressing_style(),
        )
    }
}

impl fmt::Debug for S3Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Client")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}
