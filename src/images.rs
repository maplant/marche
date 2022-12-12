use std::io::Cursor;

use aws_sdk_s3::{
    error::PutObjectError,
    model::ObjectCannedAcl,
    output::PutObjectOutput,
    types::{ByteStream, SdkError},
    Client, Endpoint,
};
use axum::body::Bytes;
use image::ImageFormat;
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::task;

pub struct Image {
    pub filename:  String,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Serialize, Error)]
pub enum UploadImageError {
    #[error("invalid file type")]
    InvalidExtension,
    #[error("error decoding image: {0}")]
    ImageError(
        #[from]
        #[serde(skip)]
        image::ImageError,
    ),
    #[error("internal server error: {0}")]
    InternalServerError(
        #[from]
        #[serde(skip)]
        tokio::task::JoinError,
    ),
    #[error("internal block storage error: {0}")]
    InternalBlockStorageError(
        #[from]
        #[serde(skip)]
        SdkError<PutObjectError>,
    ),
}

impl Image {
    /// Upload image to object storage
    pub async fn upload_image(bytes: Bytes) -> Result<Self, UploadImageError> {
        /// Maximum width/height of an image.
        const MAX_WH: u32 = 400;

        let format = image::guess_format(&bytes)?;
        let ext = match format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpeg",
            ImageFormat::Gif => "gif",
            ImageFormat::WebP => "webp",
            _ => return Err(UploadImageError::InvalidExtension),
        };

        let (bytes, hash) = task::spawn_blocking(move || {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            (
                bytes,
                base64::encode_config(hasher.finalize().as_slice(), base64::URL_SAFE_NO_PAD),
            )
        })
        .await?;

        // Check if file already exists:
        let config = aws_config::from_env()
            .endpoint_resolver(Endpoint::immutable(
                IMAGE_STORE_ENDPOINT.parse().expect("valid URI"),
            ))
            .load()
            .await;
        let client = Client::new(&config);
        let filename = format!("{hash}.{ext}");

        if image_exists(&client, &filename).await {
            let thumbnail = format!("{hash}_thumbnail.{ext}");
            return Ok(Image {
                filename:  get_url(&filename),
                thumbnail: image_exists(&client, &thumbnail)
                    .await
                    .then(move || get_url(&thumbnail)),
            });
        }

        // Resize the image if it is necessary
        let image = image::load_from_memory(&bytes)?;
        let thumbnail = if image.height() > MAX_WH || image.width() > MAX_WH {
            let thumb = task::spawn_blocking(move || image.thumbnail(MAX_WH, MAX_WH)).await?;
            let mut output = Cursor::new(Vec::with_capacity(thumb.as_bytes().len()));
            thumb.write_to(&mut output, format)?;
            let thumbnail = format!("{hash}_thumbnail.{ext}");
            put_image(
                &client,
                &thumbnail,
                &ext,
                ByteStream::from(output.into_inner()),
            )
            .await?;
            Some(thumbnail)
        } else {
            None
        };

        put_image(&client, &filename, &ext, ByteStream::from(bytes)).await?;

        Ok(Image {
            filename:  get_url(&filename),
            thumbnail: thumbnail.as_deref().map(get_url),
        })
    }
}

pub const IMAGE_STORE_ENDPOINT: &'static str = "https://marche-storage.nyc3.digitaloceanspaces.com";
pub const IMAGE_STORE_BUCKET: &'static str = "images";

pub fn get_url(filename: &str) -> String {
    format!("{IMAGE_STORE_ENDPOINT}/{IMAGE_STORE_BUCKET}/{filename}")
}

pub const MAXIMUM_FILE_SIZE: u64 = 12 * 1024 * 1024; /* 12mb */

async fn image_exists(client: &Client, filename: &str) -> bool {
    client
        .head_object()
        .bucket(IMAGE_STORE_BUCKET)
        .key(filename)
        .send()
        .await
        .is_ok()
}

async fn put_image(
    client: &Client,
    filename: &str,
    ext: &str,
    body: ByteStream,
) -> Result<PutObjectOutput, SdkError<PutObjectError>> {
    client
        .put_object()
        .acl(ObjectCannedAcl::PublicRead)
        .content_type(format!("image/{}", ext))
        .bucket(IMAGE_STORE_BUCKET)
        .key(filename)
        .body(body)
        .send()
        .await
}
