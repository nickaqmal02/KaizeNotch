// "Share my report": a read-only copy of the habit report on the web, through the same service as
// StudyNotch on the Mac (ApexAgent/cloud: Lambda Function URL + S3 + CloudFront).
//
// No account: the first time, the service gives this computer a public share id (it goes in the link)
// and a private secret. The secret lives in the system's credential store (Windows Credential Manager,
// macOS Keychain); the share id and link live in share.json next to the data. Every upload sends the
// secret; the server keeps only its hash. Only the report summary is sent (no raw sessions), at most
// every 15 minutes and only when it changed.

use crate::report;
use crate::store::{write_atomic, Store};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

pub const KEYRING_SERVICE: &str = "KaizeNotch.share";
pub const MIN_INTERVAL_SECS: i64 = 15 * 60;

// Where the share service lives. KAIZENOTCH_API overrides it (testing); https only, or a local server.
pub const DEFAULT_API: &str =
    "https://hsjsnyta2wxq6c5xrbgefn5jde0yqonm.lambda-url.ap-southeast-1.on.aws/";

pub fn api_url() -> Option<String> {
    let url = std::env::var("KAIZENOTCH_API").unwrap_or_else(|_| DEFAULT_API.to_string());
    let ok = url.starts_with("https://") || url.starts_with("http://127.0.0.1");
    (ok && !url.is_empty()).then(|| url.trim_end_matches('/').to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Saved {
    share_id: String,
    url: String,
    last_upload: Option<DateTime<Utc>>,
    last_hash: Option<String>,
}

// What the tray menu shows
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum View {
    Off,
    Working,
    On {
        url: String,
        last_upload: Option<DateTime<Utc>>,
    },
}

pub struct Share {
    api: String,
    folder: PathBuf,
    client: reqwest::Client,
    inner: Mutex<Inner>,
}

struct Inner {
    saved: Option<Saved>,
    working: bool,
    pending: bool,
    pub last_error: Option<String>,
}

#[derive(Debug)]
pub enum ShareError {
    BadReply,
    Keychain,
    Gone,
    TooBig,
    TooSoon,
    Server(u16),
    Network,
}

impl ShareError {
    pub fn message(&self) -> String {
        match self {
            ShareError::BadReply => "The share service replied with something unexpected.".into(),
            ShareError::Keychain => {
                "Could not save the link's key in the system credential store.".into()
            }
            ShareError::Gone => "This link is no longer active.".into(),
            ShareError::TooBig => "Your report is too large to share.".into(),
            ShareError::TooSoon => "Uploaded very recently; it will try again shortly.".into(),
            ShareError::Server(c) => {
                format!("The share service had a problem ({c}). It will try again later.")
            }
            ShareError::Network => {
                "Could not reach the share service. Check your internet and try again.".into()
            }
        }
    }
}

fn keyring_entry(account: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, account).ok()
}

impl Share {
    pub fn new(api: String, folder: PathBuf) -> Share {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        let saved: Option<Saved> = std::fs::read_to_string(folder.join("share.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .filter(|s: &Saved| {
                keyring_entry(&s.share_id)
                    .and_then(|e| e.get_password().ok())
                    .is_some()
            });
        Share {
            api,
            folder,
            client,
            inner: Mutex::new(Inner {
                saved,
                working: false,
                pending: false,
                last_error: None,
            }),
        }
    }

    pub fn view(&self) -> View {
        let i = self.inner.lock().unwrap();
        if i.working {
            return View::Working;
        }
        match &i.saved {
            Some(s) => View::On {
                url: s.url.clone(),
                last_upload: s.last_upload,
            },
            None => View::Off,
        }
    }

    pub fn last_error(&self) -> Option<String> {
        self.inner.lock().unwrap().last_error.clone()
    }
    pub fn is_on(&self) -> bool {
        self.inner.lock().unwrap().saved.is_some()
    }
    pub fn link(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .saved
            .as_ref()
            .map(|s| s.url.clone())
    }

    fn secret(&self) -> Option<(Saved, String)> {
        let saved = self.inner.lock().unwrap().saved.clone()?;
        let secret = keyring_entry(&saved.share_id)?.get_password().ok()?;
        Some((saved, secret))
    }

    fn persist(&self, saved: &Saved) {
        if let Ok(text) = serde_json::to_string_pretty(saved) {
            let _ = write_atomic(&self.folder.join("share.json"), text.as_bytes());
        }
    }

    fn forget(&self) {
        let mut i = self.inner.lock().unwrap();
        if let Some(s) = i.saved.take() {
            if let Some(e) = keyring_entry(&s.share_id) {
                let _ = e.delete_credential();
            }
        }
        let _ = std::fs::remove_file(self.folder.join("share.json"));
    }

    // One retry after a gateway error or a dropped connection: the service's first reply after a quiet
    // spell can time out while it wakes up
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Vec<u8>,
        auth: Option<(&str, &str)>,
    ) -> Result<serde_json::Value, ShareError> {
        match self
            .call_once(method.clone(), path, body.clone(), auth)
            .await
        {
            Err(ShareError::Network) | Err(ShareError::Server(502..=504)) => {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                self.call_once(method, path, body, auth).await
            }
            other => other,
        }
    }

    async fn call_once(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Vec<u8>,
        auth: Option<(&str, &str)>,
    ) -> Result<serde_json::Value, ShareError> {
        let mut req = self
            .client
            .request(method, format!("{}/{}", self.api, path))
            .header("Content-Type", "application/json")
            .body(body);
        if let Some((id, secret)) = auth {
            req = req
                .header("Authorization", format!("Bearer {secret}"))
                .header("X-Share-Id", id);
        }
        let resp = req.send().await.map_err(|_| ShareError::Network)?;
        match resp.status().as_u16() {
            200..=299 => Ok(resp.json().await.unwrap_or(serde_json::Value::Null)),
            403 | 404 => Err(ShareError::Gone),
            413 => Err(ShareError::TooBig),
            429 => Err(ShareError::TooSoon),
            c => Err(ShareError::Server(c)),
        }
    }

    // Turn sharing on: register, keep the secret, upload the first copy
    pub async fn start(&self) {
        {
            let mut i = self.inner.lock().unwrap();
            if i.working || i.saved.is_some() {
                return;
            }
            i.working = true;
            i.last_error = None;
        }
        let result = async {
            let body = serde_json::to_vec(
                &serde_json::json!({"app": "kaizenotch", "version": env!("CARGO_PKG_VERSION")}),
            )
            .unwrap();
            let reply = self
                .call(reqwest::Method::POST, "register", body, None)
                .await?;
            let (Some(id), Some(secret), Some(url)) = (
                reply["shareId"].as_str(),
                reply["secret"].as_str(),
                reply["url"].as_str(),
            ) else {
                return Err(ShareError::BadReply);
            };
            keyring_entry(id)
                .ok_or(ShareError::Keychain)?
                .set_password(secret)
                .map_err(|_| ShareError::Keychain)?;
            let saved = Saved {
                share_id: id.into(),
                url: url.into(),
                last_upload: None,
                last_hash: None,
            };
            self.persist(&saved);
            self.inner.lock().unwrap().saved = Some(saved);
            Ok(())
        }
        .await;
        self.inner.lock().unwrap().working = false;
        match result {
            Ok(()) => {
                self.upload(true).await;
            }
            Err(e) => self.inner.lock().unwrap().last_error = Some(e.message()),
        }
    }

    // Turn sharing off: the server deletes the copy and revokes the secret; this computer forgets both
    pub async fn stop(&self) {
        let Some((saved, secret)) = self.secret() else {
            self.forget();
            return;
        };
        self.inner.lock().unwrap().working = true;
        let res = self
            .call(
                reqwest::Method::POST,
                "stop",
                b"{}".to_vec(),
                Some((&saved.share_id, &secret)),
            )
            .await;
        let mut err = None;
        if let Err(e) = res {
            if !matches!(e, ShareError::Gone) {
                err = Some("Stopped on this computer. The page goes offline once the server is reachable again.".to_string());
            }
        }
        self.forget();
        let mut i = self.inner.lock().unwrap();
        i.working = false;
        i.last_error = err;
    }

    // Upload when the report changed. Returns how long to wait before trying again, if it is too soon.
    pub async fn upload(&self, force: bool) -> Option<Duration> {
        let (mut saved, secret) = self.secret()?;
        let data = report::json_bytes(&Store::new(&self.folder), true);
        let hash = format!("{:x}", Sha256::digest(&data));
        if !force {
            if saved.last_hash.as_deref() == Some(hash.as_str()) {
                return None;
            } // nothing new to show
            if let Some(last) = saved.last_upload {
                let age = (Utc::now() - last).num_seconds();
                if age < MIN_INTERVAL_SECS {
                    return Some(Duration::from_secs((MIN_INTERVAL_SECS - age).max(5) as u64));
                }
            }
        }
        match self
            .call(
                reqwest::Method::PUT,
                "report",
                data,
                Some((&saved.share_id, &secret)),
            )
            .await
        {
            Ok(_) => {
                saved.last_upload = Some(Utc::now());
                saved.last_hash = Some(hash);
                self.persist(&saved);
                let mut i = self.inner.lock().unwrap();
                i.saved = Some(saved);
                i.last_error = None;
                None
            }
            Err(ShareError::Gone) => {
                self.forget();
                self.inner.lock().unwrap().last_error = Some(
                    "This link was stopped on the server. Turn sharing on again for a new link."
                        .into(),
                );
                None
            }
            Err(ShareError::TooSoon) => Some(Duration::from_secs(90)),
            Err(e) => {
                self.inner.lock().unwrap().last_error = Some(e.message());
                None
            }
        }
    }

    // One delayed retry at a time
    pub fn claim_pending(&self) -> bool {
        let mut i = self.inner.lock().unwrap();
        if i.pending {
            false
        } else {
            i.pending = true;
            true
        }
    }
    pub fn release_pending(&self) {
        self.inner.lock().unwrap().pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_local_servers() {
        std::env::set_var("KAIZENOTCH_API", "http://example.com");
        assert_eq!(api_url(), None);
        std::env::set_var("KAIZENOTCH_API", "http://127.0.0.1:8787/");
        assert_eq!(api_url().as_deref(), Some("http://127.0.0.1:8787"));
        std::env::remove_var("KAIZENOTCH_API");
        assert!(api_url().unwrap().starts_with("https://"));
    }

    #[test]
    fn starts_off_without_a_saved_link() {
        let dir = tempfile::tempdir().unwrap();
        let share = Share::new("https://example.invalid".into(), dir.path().to_path_buf());
        assert_eq!(share.view(), View::Off);
        assert!(!share.is_on());
    }

    // Talks to the real share service and the system credential store, so it only runs when asked:
    //   cargo test -- --ignored live_share_round_trip
    #[test]
    #[ignore]
    fn live_share_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let api = DEFAULT_API.trim_end_matches('/').to_string();
        let share = Share::new(api, dir.path().to_path_buf());
        tauri::async_runtime::block_on(async {
            share.start().await;
            assert_eq!(share.last_error(), None);
            let View::On { url, last_upload } = share.view() else {
                panic!("sharing did not turn on")
            };
            assert!(last_upload.is_some(), "the first copy was not uploaded");
            let id = url.rsplit('/').next().unwrap().to_string();
            let data_url = url.replace(&format!("/r/{id}"), &format!("/data/{id}.json"));
            let body: serde_json::Value =
                reqwest::get(&data_url).await.unwrap().json().await.unwrap();
            assert_eq!(body["shared"], true);
            assert_eq!(body["sessions"].as_array().unwrap().len(), 0);
            // unchanged report: nothing to upload, no wait asked for
            assert_eq!(share.upload(false).await, None);
            // a second Share on the same folder finds the saved link and the secret
            assert!(Share::new(
                DEFAULT_API.trim_end_matches('/').into(),
                dir.path().to_path_buf()
            )
            .is_on());
            share.stop().await;
            assert_eq!(share.view(), View::Off);
            assert!(!dir.path().join("share.json").exists());
            let after = reqwest::get(format!("{data_url}?after=stop"))
                .await
                .unwrap()
                .status()
                .as_u16();
            assert!(
                after == 403 || after == 404,
                "the copy is still online: {after}"
            );
        });
    }
}
