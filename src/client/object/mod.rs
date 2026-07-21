mod headers;
mod listing;
mod operations;

use super::S3Client;
use crate::error::S3Error;

impl S3Client {
    fn object_operation_target(
        &self,
        key: Option<&str>,
    ) -> Result<crate::endpoint::EndpointUrl, S3Error> {
        self.inner.config.endpoint().object_url(
            self.inner.config.bucket(),
            key,
            self.inner.config.addressing_style(),
        )
    }
}
