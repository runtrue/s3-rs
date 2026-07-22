//! Atomic file download helpers built on the streaming object primitive.

use std::path::{Path, PathBuf};

use crate::error::S3Error;
use crate::operation::{GetObjectRequest, ObjectMetadata};

use super::S3Client;

/// Result of an atomic object download to a local path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadToPathOutput {
    /// Metadata returned with the object.
    pub metadata: ObjectMetadata,
    /// Number of object bytes written.
    pub bytes_written: u64,
    /// Final destination path.
    pub path: PathBuf,
}

impl S3Client {
    /// Downloads an object into a private temporary file and atomically persists
    /// it at `destination` only after the response body is fully verified.
    ///
    /// The temporary file is created in the destination directory, so the final
    /// rename cannot cross filesystems. An existing destination is atomically
    /// replaced where the platform supports replacement renames. Failed,
    /// cancelled, truncated, or checksum-invalid downloads leave the previous
    /// destination untouched and remove the temporary file on drop.
    ///
    /// # Errors
    ///
    /// Returns an error when the object request, destination I/O, integrity
    /// verification, or atomic persist operation fails.
    pub async fn download_to_path(
        &self,
        request: GetObjectRequest,
        destination: impl AsRef<Path>,
    ) -> Result<DownloadToPathOutput, S3Error> {
        let destination = destination.as_ref().to_path_buf();
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        let temporary =
            tokio::task::spawn_blocking(move || tempfile::NamedTempFile::new_in(parent))
                .await
                .map_err(|_| {
                    S3Error::cancellation("temporary download file creation was cancelled")
                })?
                .map_err(S3Error::transport)?;
        let std_file = temporary
            .as_file()
            .try_clone()
            .map_err(S3Error::transport)?;
        let mut file = tokio::fs::File::from_std(std_file);

        let output = self.get_object(request).await?;
        let bytes_written = output.body.write_to(&mut file).await?;
        file.sync_all().await.map_err(S3Error::transport)?;
        drop(file);

        // Persist directly: rename is a short filesystem operation, and keeping
        // it in this future means cancellation cannot detach a background rename
        // that later replaces the destination after the caller observed a drop.
        persist_download(temporary, &destination)?;

        Ok(DownloadToPathOutput {
            metadata: output.metadata,
            bytes_written,
            path: destination,
        })
    }
}

fn persist_download(temporary: tempfile::NamedTempFile, destination: &Path) -> Result<(), S3Error> {
    temporary
        .persist(destination)
        .map(|_| ())
        .map_err(|error| S3Error::transport(error.error))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn atomic_persist_replaces_an_existing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("object");
        std::fs::write(&destination, b"old").unwrap();
        let mut temporary = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        temporary.write_all(b"new").unwrap();

        persist_download(temporary, &destination).unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), b"new");
    }
}
