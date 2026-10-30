//! Publish immutable bundles first, then switch the device manifest last.
mod local;
mod r2;

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::storage::Storage;

pub const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";
pub const LATEST_CACHE: &str = "no-store, max-age=0";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectMetadata {
    pub sha256: String,
    pub size: usize,
    pub revision: String,
}

pub struct Object {
    pub key: String,
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub cache_control: &'static str,
    pub metadata: ObjectMetadata,
}

#[derive(Debug)]
pub struct UploadError {
    pub message: String,
    pub retryable: bool,
}

impl UploadError {
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

pub trait Publisher {
    async fn head(&self, key: &str) -> Result<Option<ObjectMetadata>, UploadError>;
    async fn put(&self, object: &Object) -> Result<(), UploadError>;
}

pub struct Bundle {
    destination: String,
    channel: String,
    revision: String,
    image_id: String,
    objects: Vec<Object>,
    latest: Object,
}

pub fn enabled() -> Result<bool, String> {
    match env::var("FLIGHT_PUBLISH_ENABLED").as_deref() {
        Ok("true" | "1") => Ok(true),
        Ok("false" | "0" | "") | Err(_) => Ok(false),
        _ => Err("FLIGHT_PUBLISH_ENABLED must be true or false".into()),
    }
}

fn public_base() -> Result<Url, String> {
    let input = env::var("R2_PUBLIC_BASE_URL")
        .map_err(|_| "set R2_PUBLIC_BASE_URL to your bucket's public HTTPS URL")?;
    validate_base(&input)
}

fn validate_base(input: &str) -> Result<Url, String> {
    let url = Url::parse(&format!("{}/", input.trim_end_matches('/')))
        .map_err(|_| "invalid R2_PUBLIC_BASE_URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "R2_PUBLIC_BASE_URL must be an HTTPS base URL without credentials, query, or fragment"
                .into(),
        );
    }
    Ok(url)
}

pub async fn publish_from_env(
    storage: &Storage,
    directory: &Path,
    channel: &str,
    revision: &str,
    data: &Path,
) -> Result<(), String> {
    let _lock = publication_lock(&data.join("publication.lock"))?;
    let base = public_base()?;
    match env::var("FLIGHT_PUBLISH_BACKEND")
        .as_deref()
        .unwrap_or("r2")
    {
        "local" => {
            let root = env::var_os("FLIGHT_LOCAL_PUBLISH_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data.join("published"));
            fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let destination = format!(
                "local:{}:{}",
                root.canonicalize().map_err(|e| e.to_string())?.display(),
                base
            );
            let bundle = Bundle::load(directory, channel, revision, &base, &destination)?;
            publish(storage, &local::LocalPublisher::new(root), &bundle).await
        }
        "r2" => {
            let (publisher, destination) = r2::R2Publisher::from_env(&base)?;
            let bundle = Bundle::load(directory, channel, revision, &base, &destination)?;
            publish(storage, &publisher, &bundle).await
        }
        _ => Err("FLIGHT_PUBLISH_BACKEND must be r2 or local".into()),
    }
}

fn publication_lock(path: &Path) -> Result<fs::File, String> {
    use fs2::FileExt;
    fs::create_dir_all(path.parent().ok_or("invalid publication lock path")?)
        .map_err(|e| e.to_string())?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.try_lock_exclusive()
        .map_err(|_| "another publication is active; retry shortly")?;
    Ok(file)
}

impl Bundle {
    pub fn load(
        directory: &Path,
        channel: &str,
        revision: &str,
        base: &Url,
        destination: &str,
    ) -> Result<Self, String> {
        match channel {
            "daily" => {
                crate::service::parse_date(revision)?;
            }
            "test" if revision.len() >= 6 && revision.chars().all(|c| c.is_ascii_digit()) => {
                revision
                    .parse::<u64>()
                    .map_err(|_| "invalid test revision")?;
            }
            _ => return Err("invalid publication channel or revision".into()),
        }
        let binary = read(directory, "display.bin")?;
        let preview = read(directory, "preview.png")?;
        let json = read(directory, "flight.json")?;
        let mut manifest: Value =
            serde_json::from_slice(&json).map_err(|e| format!("invalid flight.json: {e}"))?;
        let binary_hash = sha256(&binary);
        if binary.len() != 48_000
            || manifest["display"]["width"] != 800
            || manifest["display"]["height"] != 480
            || manifest["display"]["binary_size"] != 48_000
            || manifest["artifacts"]["binary_sha256"] != binary_hash
        {
            return Err(
                "local binary failed size, display dimensions, or SHA-256 validation".into(),
            );
        }
        if channel == "daily" && manifest["date"] != revision {
            return Err("manifest date does not match publication date".into());
        }
        let image_id = sha256(
            &serde_json::to_vec(&(binary_hash, sha256(&preview), sha256(&json), base.as_str()))
                .map_err(|e| e.to_string())?,
        );
        let prefix = format!("{channel}/{revision}/{image_id}");
        let absolute = |name: &str| -> Result<String, String> {
            Ok(base
                .join(&format!("{prefix}/{name}"))
                .map_err(|e| e.to_string())?
                .to_string())
        };
        manifest["image_id"] = image_id.clone().into();
        manifest["channel"] = channel.into();
        manifest["revision"] = revision.into();
        manifest["binary_url"] = absolute("display.bin")?.into();
        manifest["preview_url"] = absolute("preview.png")?.into();
        manifest["manifest_url"] = absolute("flight.json")?.into();
        manifest["artifacts"]["binary"] = absolute("display.bin")?.into();
        manifest["artifacts"]["preview"] = absolute("preview.png")?.into();
        let manifest = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        let objects = vec![
            Object::new(
                format!("{prefix}/display.bin"),
                binary,
                "application/octet-stream",
                IMMUTABLE_CACHE,
                revision,
            ),
            Object::new(
                format!("{prefix}/preview.png"),
                preview,
                "image/png",
                IMMUTABLE_CACHE,
                revision,
            ),
            Object::new(
                format!("{prefix}/flight.json"),
                manifest.clone(),
                "application/json",
                IMMUTABLE_CACHE,
                revision,
            ),
        ];
        let latest = Object::new(
            format!("{channel}/latest.json"),
            manifest,
            "application/json",
            LATEST_CACHE,
            revision,
        );
        Ok(Self {
            destination: destination.into(),
            channel: channel.into(),
            revision: revision.into(),
            image_id,
            objects,
            latest,
        })
    }
}

impl Object {
    fn new(
        key: String,
        bytes: Vec<u8>,
        content_type: &'static str,
        cache_control: &'static str,
        revision: &str,
    ) -> Self {
        let metadata = ObjectMetadata {
            sha256: sha256(&bytes),
            size: bytes.len(),
            revision: revision.into(),
        };
        Self {
            key,
            bytes,
            content_type,
            cache_control,
            metadata,
        }
    }
}

fn read(root: &Path, name: &str) -> Result<Vec<u8>, String> {
    fs::read(root.join(name)).map_err(|e| format!("cannot read {}: {e}", root.join(name).display()))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

async fn retry_head(
    publisher: &impl Publisher,
    key: &str,
) -> Result<Option<ObjectMetadata>, String> {
    for attempt in 0..3 {
        match publisher.head(key).await {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable && attempt < 2 => {
                tokio::time::sleep(Duration::from_millis(250 << attempt)).await
            }
            Err(error) => return Err(error.message),
        }
    }
    unreachable!()
}

async fn retry_put(publisher: &impl Publisher, object: &Object) -> Result<(), String> {
    for attempt in 0..3 {
        match publisher.put(object).await {
            Ok(()) => return Ok(()),
            Err(error) if error.retryable && attempt < 2 => {
                tokio::time::sleep(Duration::from_millis(250 << attempt)).await
            }
            Err(error) => return Err(error.message),
        }
    }
    unreachable!()
}

async fn verify(publisher: &impl Publisher, object: &Object) -> Result<(), String> {
    let head = retry_head(publisher, &object.key)
        .await?
        .ok_or_else(|| format!("object missing after upload: {}", object.key))?;
    if head.sha256 != object.metadata.sha256 || head.size != object.metadata.size {
        return Err(format!("upload verification failed: {}", object.key));
    }
    Ok(())
}

async fn publish(
    storage: &Storage,
    publisher: &impl Publisher,
    bundle: &Bundle,
) -> Result<(), String> {
    storage
        .publication_state(
            &bundle.destination,
            &bundle.channel,
            &bundle.revision,
            &bundle.image_id,
            ("uploading", None),
        )
        .await
        .map_err(|e| e.to_string())?;
    let result = publish_inner(storage, publisher, bundle).await;
    if let Err(error) = &result {
        storage
            .publication_state(
                &bundle.destination,
                &bundle.channel,
                &bundle.revision,
                &bundle.image_id,
                ("failed", Some(error)),
            )
            .await
            .map_err(|e| format!("{error}; cannot record failure: {e}"))?;
    }
    result
}

async fn publish_inner(
    storage: &Storage,
    publisher: &impl Publisher,
    bundle: &Bundle,
) -> Result<(), String> {
    for object in &bundle.objects {
        match retry_head(publisher, &object.key).await? {
            Some(remote)
                if remote.sha256 == object.metadata.sha256
                    && remote.size == object.metadata.size => {}
            Some(_) => {
                return Err(format!(
                    "immutable object checksum conflict: {}",
                    object.key
                ));
            }
            None => {
                retry_put(publisher, object).await?;
                verify(publisher, object).await?;
            }
        }
        storage
            .record_upload(
                &bundle.destination,
                &object.key,
                &object.metadata.sha256,
                object.metadata.size,
            )
            .await
            .map_err(|e| e.to_string())?;
    }
    let current = retry_head(publisher, &bundle.latest.key).await?;
    if let Some(remote) = &current {
        let newer = match bundle.channel.as_str() {
            "daily" => {
                crate::service::parse_date(&remote.revision)?
                    > crate::service::parse_date(&bundle.revision)?
            }
            _ => {
                remote
                    .revision
                    .parse::<u64>()
                    .map_err(|_| "invalid remote revision")?
                    > bundle
                        .revision
                        .parse::<u64>()
                        .map_err(|_| "invalid test revision")?
            }
        };
        if newer {
            storage
                .publication_state(
                    &bundle.destination,
                    &bundle.channel,
                    &bundle.revision,
                    &bundle.image_id,
                    ("archived", None),
                )
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "Archived {} {}; newer latest manifest retained",
                bundle.channel, bundle.revision
            );
            return Ok(());
        }
    }
    if current.is_none_or(|remote| {
        remote.sha256 != bundle.latest.metadata.sha256 || remote.size != bundle.latest.metadata.size
    }) {
        retry_put(publisher, &bundle.latest).await?;
    }
    verify(publisher, &bundle.latest).await?;
    storage
        .record_upload(
            &bundle.destination,
            &bundle.latest.key,
            &bundle.latest.metadata.sha256,
            bundle.latest.metadata.size,
        )
        .await
        .map_err(|e| e.to_string())?;
    storage
        .record_publication_head(
            &bundle.destination,
            &bundle.channel,
            &bundle.revision,
            &bundle.image_id,
        )
        .await
        .map_err(|e| e.to_string())?;
    println!(
        "Published {} {} image_id={}",
        bundle.channel, bundle.revision, bundle.image_id
    );
    Ok(())
}

pub fn metadata_map(object: &Object) -> HashMap<String, String> {
    HashMap::from([
        ("sha256".into(), object.metadata.sha256.clone()),
        ("revision".into(), object.metadata.revision.clone()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockState {
        objects: HashMap<String, (Vec<u8>, ObjectMetadata)>,
        puts: Vec<String>,
        fail_suffix: Option<String>,
        transient_failures: usize,
        attempts: usize,
    }
    #[derive(Default)]
    struct MockPublisher(Mutex<MockState>);

    impl Publisher for MockPublisher {
        async fn head(&self, key: &str) -> Result<Option<ObjectMetadata>, UploadError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .objects
                .get(key)
                .map(|(_, meta)| meta.clone()))
        }
        async fn put(&self, object: &Object) -> Result<(), UploadError> {
            let mut state = self.0.lock().unwrap();
            state.attempts += 1;
            if state.transient_failures > 0 {
                state.transient_failures -= 1;
                return Err(UploadError {
                    message: "simulated 503".into(),
                    retryable: true,
                });
            }
            if state
                .fail_suffix
                .as_ref()
                .is_some_and(|suffix| object.key.ends_with(suffix))
            {
                return Err(UploadError::permanent("simulated upload failure"));
            }
            state.puts.push(object.key.clone());
            state.objects.insert(
                object.key.clone(),
                (object.bytes.clone(), object.metadata.clone()),
            );
            Ok(())
        }
    }

    #[tokio::test]
    async fn publication_resumes_after_failure_and_channels_do_not_interfere() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "publication-test-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ));
        let directory = root.join("artifacts");
        crate::artifact::render_fixture(&directory, crate::artifact::DisplayProfile::default())
            .unwrap();
        let storage = Storage::connect(&root.join("state.db")).await.unwrap();
        let base = validate_base("https://display.example.com").unwrap();
        let bundle = Bundle::load(&directory, "daily", "2026-10-29", &base, "mock").unwrap();
        let mock = MockPublisher::default();
        let old_pointer = b"previous published manifest".to_vec();
        mock.0.lock().unwrap().objects.insert(
            "daily/latest.json".into(),
            (
                old_pointer.clone(),
                ObjectMetadata {
                    sha256: sha256(&old_pointer),
                    size: old_pointer.len(),
                    revision: "2026-10-28".into(),
                },
            ),
        );
        mock.0.lock().unwrap().fail_suffix = Some("/preview.png".into());
        assert!(publish(&storage, &mock, &bundle).await.is_err());
        assert_eq!(
            mock.0.lock().unwrap().objects["daily/latest.json"].0,
            old_pointer
        );
        assert_eq!(
            storage.publication_status().await.unwrap()["publications"][0]["status"],
            "failed"
        );
        mock.0.lock().unwrap().fail_suffix = None;
        mock.0.lock().unwrap().transient_failures = 1;
        publish(&storage, &mock, &bundle).await.unwrap();
        {
            let state = mock.0.lock().unwrap();
            assert_eq!(
                state
                    .puts
                    .iter()
                    .filter(|key| key.ends_with("/display.bin"))
                    .count(),
                1
            );
            assert_eq!(state.puts.last().unwrap(), "daily/latest.json");
            let remote: Value =
                serde_json::from_slice(&state.objects["daily/latest.json"].0).unwrap();
            assert_eq!(remote["display"]["binary_size"], 48_000);
            assert!(
                remote["binary_url"]
                    .as_str()
                    .unwrap()
                    .starts_with("https://display.example.com/daily/")
            );
            assert_eq!(state.objects[&bundle.objects[0].key].0.len(), 48_000);
            assert_eq!(
                sha256(&state.objects[&bundle.objects[0].key].0),
                remote["artifacts"]["binary_sha256"]
            );
        }
        let puts = mock.0.lock().unwrap().puts.len();
        publish(&storage, &mock, &bundle).await.unwrap();
        assert_eq!(puts, mock.0.lock().unwrap().puts.len());
        let test_bundle = Bundle::load(&directory, "test", "000001", &base, "mock").unwrap();
        publish(&storage, &mock, &test_bundle).await.unwrap();
        assert!(
            mock.0
                .lock()
                .unwrap()
                .objects
                .contains_key("test/latest.json")
        );
        assert_eq!(
            mock.0.lock().unwrap().objects["daily/latest.json"].1.sha256,
            bundle.latest.metadata.sha256
        );

        // After a newer day is published, a backfill must not replace the pointer.
        mock.0
            .lock()
            .unwrap()
            .objects
            .get_mut("daily/latest.json")
            .unwrap()
            .1
            .revision = "2026-10-30".into();
        publish(&storage, &mock, &bundle).await.unwrap();
        assert_eq!(
            mock.0.lock().unwrap().objects["daily/latest.json"]
                .1
                .revision,
            "2026-10-30"
        );
        fs::write(directory.join("display.bin"), b"corrupt").unwrap();
        assert!(Bundle::load(&directory, "test", "000002", &base, "mock").is_err());
        storage.close().await;
        for attempt in 0..40 {
            match fs::remove_dir_all(&root) {
                Ok(()) => break,
                Err(_) if attempt < 39 => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(error) => panic!("publication test cleanup: {error}"),
            }
        }
    }
}
