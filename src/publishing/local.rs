use std::{fs, path::PathBuf};

use super::{Object, ObjectMetadata, Publisher, UploadError, sha256};

pub struct LocalPublisher {
    root: PathBuf,
}
impl LocalPublisher {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Publisher for LocalPublisher {
    async fn head(&self, key: &str) -> Result<Option<ObjectMetadata>, UploadError> {
        let path = self.root.join(key);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(UploadError::permanent(error.to_string())),
        };
        let metadata = fs::read(path.with_extension("metadata.json"))
            .map_err(|e| UploadError::permanent(e.to_string()))?;
        let mut metadata: ObjectMetadata =
            serde_json::from_slice(&metadata).map_err(|e| UploadError::permanent(e.to_string()))?;
        metadata.sha256 = sha256(&bytes);
        metadata.size = bytes.len();
        Ok(Some(metadata))
    }

    async fn put(&self, object: &Object) -> Result<(), UploadError> {
        let path = self.root.join(&object.key);
        fs::create_dir_all(path.parent().unwrap())
            .map_err(|e| UploadError::permanent(e.to_string()))?;
        let temporary = path.with_extension("upload.tmp");
        fs::write(&temporary, &object.bytes).map_err(|e| UploadError::permanent(e.to_string()))?;
        fs::rename(&temporary, &path).map_err(|e| UploadError::permanent(e.to_string()))?;
        fs::write(
            path.with_extension("metadata.json"),
            serde_json::to_vec(&object.metadata)
                .map_err(|e| UploadError::permanent(e.to_string()))?,
        )
        .map_err(|e| UploadError::permanent(e.to_string()))?;
        Ok(())
    }
}
