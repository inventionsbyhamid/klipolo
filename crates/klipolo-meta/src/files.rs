//! The Files API: upload media once, refer to it by id, delete it after.

use std::path::Path;

use reqwest::multipart::{Form, Part};
use reqwest::{Body, StatusCode};
use serde::Deserialize;

use crate::client::check;
use crate::error::redact;
use crate::{MetaClient, MetaError};

/// Meta deletes an upload after a day even if the app never gets to.
const UPLOAD_EXPIRY_SECONDS: u64 = 24 * 60 * 60;

#[derive(Deserialize)]
struct Uploaded {
    id: String,
}

impl MetaClient {
    /// Uploads a file (purpose `user_data`, e.g. an MP4 for `input_video`)
    /// and returns its id. The file is streamed, never read whole. Meta sees
    /// a generic name (`upload.mp4`), not the person's file name.
    pub async fn upload_file(&self, path: &Path, mime: &str) -> Result<String, MetaError> {
        let unreadable = |_| self.unreadable(path);
        let file = tokio::fs::File::open(path).await.map_err(unreadable)?;
        let len = file.metadata().await.map_err(unreadable)?.len();
        let name = match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => format!("upload.{ext}"),
            None => "upload".to_owned(),
        };
        let file = Part::stream_with_length(Body::from(file), len)
            .file_name(name)
            .mime_str(mime)
            .map_err(|_| MetaError::BadRequest {
                message: format!("{mime} is not a media type."),
            })?;
        let form = Form::new()
            .text("purpose", "user_data")
            .text("expires_after[anchor]", "created_at")
            .text("expires_after[seconds]", UPLOAD_EXPIRY_SECONDS.to_string())
            .part("file", file);
        let request = self.http.post(self.meta_url("files")).multipart(form);
        let response = self.send_meta(request).await?;
        let uploaded: Uploaded = self.read_json(response).await?;
        Ok(uploaded.id)
    }

    /// Deletes an uploaded file. A file that is already gone (deleted or
    /// expired) counts as deleted.
    pub async fn delete_file(&self, id: &str) -> Result<(), MetaError> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(MetaError::BadRequest {
                message: "This file id is not valid.".into(),
            });
        }
        let request = self
            .http
            .delete(self.meta_url(&format!("files/{id}")))
            .timeout(self.short_timeout);
        let response = self.send_meta_unchecked(request).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(response, &self.secrets(), self.short_timeout).await?;
        Ok(())
    }
}

impl MetaClient {
    /// The file's name only: no folders, and no OS error text, for people.
    fn unreadable(&self, path: &Path) -> MetaError {
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        MetaError::LocalFile {
            name: redact(&name, &self.secrets()),
        }
    }
}
