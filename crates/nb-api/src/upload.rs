//! Content-addressed asset upload: `prepareMediaUpload`, then a cookie-less PUT to the signed URL.

use base64::Engine;
use md5::Digest as _;
use serde_json::{Value, json};

use crate::error::{NbError, Result};
use crate::http::Http;

const PREPARE: &str = "
mutation prepareMediaUpload($prepareMediaUploadParams: PrepareMediaUploadParams!) {
  prepareMediaUpload(prepareMediaUploadParams: $prepareMediaUploadParams) { uploadStatus mediaUploadSignedUrl }
}";

pub struct AssetDigest {
    pub sha512: String,
    pub md5_b64: String,
    pub size: usize,
}

pub fn digest(data: &[u8]) -> AssetDigest {
    let sha = sha2::Sha512::digest(data);
    AssetDigest {
        sha512: sha.iter().map(|b| format!("{b:02x}")).collect(),
        md5_b64: base64::engine::general_purpose::STANDARD.encode(md5::Md5::digest(data)),
        size: data.len(),
    }
}

impl Http {
    /// Returns `{sha512, size, status, uploaded}`; already-known content is not re-sent.
    pub async fn upload_bytes(&self, data: &[u8]) -> Result<Value> {
        let d = digest(data);
        let vars = json!({"prepareMediaUploadParams": {"md5Hash": d.md5_b64, "sha512Hash": d.sha512, "sizeInBytes": d.size}});
        let info = self.gql_field(PREPARE, vars, "prepareMediaUpload").await?;
        let status = info["uploadStatus"].clone();
        let mut uploaded = false;
        if let Some(url) = info["mediaUploadSignedUrl"]
            .as_str()
            .filter(|_| status != "UPLOADED")
        {
            let headers = [
                ("Content-MD5", d.md5_b64.clone()),
                ("Content-Type", "application/octet-stream".to_string()),
                ("x-goog-content-length-range", format!("0,{}", d.size)),
            ];
            let r = self.put_external(url, &headers, data.to_vec()).await?;
            if !matches!(r.status, 200 | 201) {
                let t: String = r.text().chars().take(200).collect();
                return Err(NbError::generic(format!(
                    "asset upload failed: HTTP {} {t}",
                    r.status
                )));
            }
            uploaded = true;
        }
        Ok(json!({"sha512": d.sha512, "size": d.size, "status": status, "uploaded": uploaded}))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn digest_matches_reference() {
        let d = super::digest(b"abc");
        assert_eq!(d.md5_b64, "kAFQmDzST7DWlj99KOF/cg==");
        assert!(d.sha512.starts_with("ddaf35a193617aba"));
        assert_eq!(d.size, 3);
    }
}
