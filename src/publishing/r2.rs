use std::{env, fs, time::Duration};

use aws_sdk_s3::{
    Client,
    config::{
        BehaviorVersion, Credentials, Region, RequestChecksumCalculation, retry::RetryConfig,
        timeout::TimeoutConfig,
    },
    primitives::ByteStream,
};
use url::Url;

use super::{Object, ObjectMetadata, Publisher, UploadError, metadata_map};

pub struct R2Publisher {
    client: Client,
    bucket: String,
}

impl R2Publisher {
    pub fn from_env(public_base: &Url) -> Result<(Self, String), String> {
        let endpoint =
            env::var("R2_ENDPOINT").map_err(|_| "set R2_ENDPOINT to the account S3 endpoint")?;
        let parsed = Url::parse(&endpoint).map_err(|_| "invalid R2_ENDPOINT")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err(
                "R2_ENDPOINT must be an HTTPS origin without credentials, path, query, or fragment"
                    .into(),
            );
        }
        let bucket = env::var("R2_BUCKET").map_err(|_| "set R2_BUCKET")?;
        if bucket.is_empty()
            || !bucket
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
        {
            return Err("invalid R2_BUCKET".into());
        }
        let access_key = secret("R2_ACCESS_KEY_ID")?;
        let secret_key = secret("R2_SECRET_ACCESS_KEY")?;
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .http_client(
                aws_smithy_http_client::Builder::new()
                    .tls_provider(aws_smithy_http_client::tls::Provider::Rustls(
                        aws_smithy_http_client::tls::rustls_provider::CryptoMode::Ring,
                    ))
                    .build_https(),
            )
            .region(Region::new("auto"))
            .endpoint_url(&endpoint)
            .force_path_style(true)
            .credentials_provider(Credentials::new(
                access_key,
                secret_key,
                None,
                None,
                "flight-a-day-r2",
            ))
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .operation_timeout(Duration::from_secs(30))
                    .build(),
            )
            .build();
        let destination = format!(
            "r2:{}/{bucket}:{public_base}",
            endpoint.trim_end_matches('/')
        );
        Ok((
            Self {
                client: Client::from_conf(config),
                bucket,
            },
            destination,
        ))
    }
}

fn secret(name: &str) -> Result<String, String> {
    let value = match env::var(format!("{name}_FILE")) {
        Ok(path) if !path.is_empty() => {
            fs::read_to_string(path).map_err(|_| format!("could not read {name}_FILE"))?
        }
        _ => env::var(name).map_err(|_| format!("set {name} or {name}_FILE"))?,
    };
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(format!("{name} is empty"));
    }
    Ok(value)
}

impl Publisher for R2Publisher {
    async fn head(&self, key: &str) -> Result<Option<ObjectMetadata>, UploadError> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(output) => {
                let metadata = output.metadata();
                let sha256 = metadata
                    .and_then(|m| m.get("sha256"))
                    .cloned()
                    .unwrap_or_default();
                let revision = metadata
                    .and_then(|m| m.get("revision"))
                    .cloned()
                    .unwrap_or_default();
                let size = output
                    .content_length()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| {
                        UploadError::permanent("R2 HEAD response has no valid content length")
                    })?;
                Ok(Some(ObjectMetadata {
                    sha256,
                    revision,
                    size,
                }))
            }
            Err(error) => {
                let status = error
                    .raw_response()
                    .map(|response| response.status().as_u16());
                if status == Some(404) {
                    return Ok(None);
                }
                Err(request_error("HEAD", status))
            }
        }
    }

    async fn put(&self, object: &Object) -> Result<(), UploadError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&object.key)
            .body(ByteStream::from(object.bytes.clone()))
            .content_type(object.content_type)
            .cache_control(object.cache_control)
            .set_metadata(Some(metadata_map(object)))
            .send()
            .await
            .map_err(|error| {
                request_error(
                    "PUT",
                    error
                        .raw_response()
                        .map(|response| response.status().as_u16()),
                )
            })?;
        Ok(())
    }
}

fn request_error(operation: &str, status: Option<u16>) -> UploadError {
    // SDK debug output can contain request details; expose only method/status.
    UploadError {
        message: format!(
            "R2 {operation} failed ({})",
            status.map_or_else(|| "transport/timeout".into(), |code| code.to_string())
        ),
        retryable: status.is_none_or(|status| status == 429 || status == 408 || status >= 500),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        io::{Read, Write},
        net::TcpListener,
    };

    #[tokio::test]
    async fn sdk_sends_signed_put_and_verifies_head_metadata() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let object = Object::new(
            "test/000001/display.bin".into(),
            vec![255; 48_000],
            "application/octet-stream",
            super::super::IMMUTABLE_CACHE,
            "000001",
        );
        let expected_hash = object.metadata.sha256.clone();
        let server = std::thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let header_end = loop {
                    let mut buffer = [0; 4096];
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(position) =
                        request.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        break position + 4;
                    }
                };
                let header = String::from_utf8_lossy(&request[..header_end]).to_string();
                let headers: HashMap<_, _> = header
                    .lines()
                    .skip(1)
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
                    .collect();
                assert!(headers["authorization"].starts_with("AWS4-HMAC-SHA256"));
                if index == 0 {
                    assert!(header.starts_with("PUT /demo-bucket/test/000001/display.bin"));
                    assert_eq!(headers["cache-control"], super::super::IMMUTABLE_CACHE);
                    assert_eq!(headers["content-type"], "application/octet-stream");
                    assert_eq!(headers["x-amz-meta-sha256"], expected_hash);
                    let length = headers["content-length"].parse::<usize>().unwrap();
                    while request.len() < header_end + length {
                        let mut buffer = [0; 4096];
                        let count = stream.read(&mut buffer).unwrap();
                        assert!(count > 0);
                        request.extend_from_slice(&buffer[..count]);
                    }
                    assert_eq!(
                        super::super::sha256(&request[header_end..header_end + length]),
                        expected_hash
                    );
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .unwrap();
                } else if index == 1 {
                    assert!(header.starts_with("HEAD /demo-bucket/test/000001/display.bin"));
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 48000\r\nx-amz-meta-sha256: {expected_hash}\r\nx-amz-meta-revision: 000001\r\nConnection: close\r\n\r\n").unwrap();
                } else {
                    stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                }
            }
        });
        let client = aws_smithy_http_client::Builder::new()
            .tls_provider(aws_smithy_http_client::tls::Provider::Rustls(
                aws_smithy_http_client::tls::rustls_provider::CryptoMode::Ring,
            ))
            .build_https();
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .http_client(client)
            .region(Region::new("auto"))
            .endpoint_url(format!("http://{address}"))
            .force_path_style(true)
            .credentials_provider(Credentials::new(
                "test-access",
                "test-secret",
                None,
                None,
                "test",
            ))
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(10))
                    .build(),
            )
            .build();
        let publisher = R2Publisher {
            client: Client::from_conf(config),
            bucket: "demo-bucket".into(),
        };
        publisher.put(&object).await.unwrap();
        let metadata = publisher.head(&object.key).await.unwrap().unwrap();
        assert_eq!(metadata.sha256, object.metadata.sha256);
        assert_eq!(metadata.size, 48_000);
        assert!(publisher.head("missing").await.unwrap().is_none());
        server.join().unwrap();
    }
}
