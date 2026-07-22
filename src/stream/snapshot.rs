use std::path::{Path, PathBuf};

use bytes::{Bytes, BytesMut};
use sha2::{Digest as _, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use crate::error::S3Error;

pub(crate) struct FileSnapshot {
    path: tempfile::TempPath,
    length: u64,
    sha256: Option<[u8; 32]>,
}

impl FileSnapshot {
    pub(crate) async fn create(path: PathBuf, hash: bool) -> Result<Self, S3Error> {
        let mut input = File::open(&path).await.map_err(S3Error::transport)?;
        let metadata = input.metadata().await.map_err(S3Error::transport)?;
        if !metadata.is_file() {
            return Err(S3Error::configuration(
                "upload path must identify a regular file",
            ));
        }

        let snapshot = tempfile::NamedTempFile::new().map_err(S3Error::transport)?;
        let (snapshot_file, snapshot_path) = snapshot.into_parts();
        let mut output = File::from_std(snapshot_file);
        let mut hasher = hash.then(Sha256::new);
        let mut length = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer).await.map_err(S3Error::transport)?;
            if read == 0 {
                break;
            }
            length = length
                .checked_add(u64::try_from(read).map_err(|_| {
                    S3Error::integrity("upload file chunk length does not fit in u64")
                })?)
                .ok_or_else(|| S3Error::integrity("upload file length overflow"))?;
            if let Some(hasher) = &mut hasher {
                hasher.update(&buffer[..read]);
            }
            output
                .write_all(&buffer[..read])
                .await
                .map_err(S3Error::transport)?;
        }
        output.flush().await.map_err(S3Error::transport)?;
        drop(output);

        make_read_only(&snapshot_path)?;
        let actual = tokio::fs::metadata(&snapshot_path)
            .await
            .map_err(S3Error::transport)?
            .len();
        if actual != length {
            return Err(S3Error::integrity(
                "upload snapshot length changed while preparing",
            ));
        }

        Ok(Self {
            path: snapshot_path,
            length,
            sha256: hasher.map(|hasher| hasher.finalize().into()),
        })
    }

    pub(crate) const fn length(&self) -> u64 {
        self.length
    }

    pub(crate) fn sha256(&self) -> Option<[u8; 32]> {
        self.sha256
    }

    pub(crate) async fn open(&self) -> Result<File, S3Error> {
        File::open(&self.path).await.map_err(S3Error::transport)
    }

    pub(crate) async fn read_range(&self, offset: u64, length: usize) -> Result<Bytes, S3Error> {
        let mut file = self.open().await?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(S3Error::transport)?;
        let mut bytes = BytesMut::zeroed(length);
        file.read_exact(&mut bytes)
            .await
            .map_err(S3Error::transport)?;
        Ok(bytes.freeze())
    }
}

fn make_read_only(path: &Path) -> Result<(), S3Error> {
    let mut permissions = std::fs::metadata(path)
        .map_err(S3Error::transport)?
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions).map_err(S3Error::transport)
}
