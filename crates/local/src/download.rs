//! Downloading model files: resumable, checksummed, with progress.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::LocalError;
use crate::hardware::free_space;
use crate::hf::{Hub, RemoteFile, dir_name};

/// Written last; a model folder without it is an unfinished download.
pub const MANIFEST: &str = "chummy-model.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub repo: String,
    pub revision: String,
    pub parser: String,
    pub files: Vec<String>,
    pub bytes: u64,
    pub downloaded_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub repo: String,
    pub file: String,
    pub file_index: usize,
    pub file_count: usize,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub bytes_per_sec: f64,
}

/// What to download.
pub struct Request<'a> {
    pub repo: &'a str,
    pub revision: &'a str,
    pub parser: &'a str,
    pub files: &'a [RemoteFile],
}

/// Downloads `req` into `models_dir/<owner>--<name>/` and returns that folder.
/// Interrupted downloads resume from `*.part` files on the next call.
pub async fn download(
    hub: &Hub,
    req: Request<'_>,
    models_dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(&Progress),
) -> Result<PathBuf, LocalError> {
    let dest = models_dir.join(dir_name(req.repo));
    tokio::fs::create_dir_all(&dest)
        .await
        .map_err(LocalError::io("creating the model folder"))?;

    let total: u64 = req.files.iter().map(|f| f.size).sum();
    let mut already = 0u64;
    for f in req.files {
        if let Ok(m) = std::fs::metadata(dest.join(&f.path)) {
            if m.len() == f.size {
                already += f.size;
            }
        } else if let Ok(m) = std::fs::metadata(part_path(&dest, &f.path)) {
            already += m.len().min(f.size);
        }
    }
    let need = total.saturating_sub(already) + 512 * 1024 * 1024;
    if let Some(free) = free_space(models_dir)
        && free < need
    {
        return Err(LocalError::DiskSpace {
            need_gb: need as f64 / 1e9,
            have_gb: free as f64 / 1e9,
        });
    }

    let started = Instant::now();
    let mut done = 0u64;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut progress = Progress {
        repo: req.repo.into(),
        file: String::new(),
        file_index: 0,
        file_count: req.files.len(),
        done_bytes: 0,
        total_bytes: total,
        bytes_per_sec: 0.0,
    };

    for (i, f) in req.files.iter().enumerate() {
        progress.file = f.path.clone();
        progress.file_index = i;
        let final_path = dest.join(&f.path);
        if std::fs::metadata(&final_path).is_ok_and(|m| m.len() == f.size) {
            done += f.size;
            continue;
        }
        let base_done = done;
        fetch_file(hub, &req, f, &dest, cancel, |file_bytes| {
            done = base_done + file_bytes;
            if last_emit.elapsed() >= Duration::from_millis(200) {
                last_emit = Instant::now();
                progress.done_bytes = done;
                let secs = started.elapsed().as_secs_f64().max(0.001);
                progress.bytes_per_sec = done.saturating_sub(already) as f64 / secs;
                on_progress(&progress);
            }
        })
        .await?;
        done = base_done + f.size;
    }

    let manifest = Manifest {
        repo: req.repo.into(),
        revision: req.revision.into(),
        parser: req.parser.into(),
        files: req.files.iter().map(|f| f.path.clone()).collect(),
        bytes: total,
        downloaded_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let tmp = dest.join(format!("{MANIFEST}.tmp"));
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(LocalError::io("writing the model manifest"))?;
    std::fs::rename(&tmp, dest.join(MANIFEST))
        .map_err(LocalError::io("writing the model manifest"))?;

    progress.done_bytes = total;
    progress.file_index = req.files.len();
    on_progress(&progress);
    Ok(dest)
}

fn part_path(dest: &Path, file: &str) -> PathBuf {
    dest.join(format!("{file}.part"))
}

async fn fetch_file(
    hub: &Hub,
    req: &Request<'_>,
    f: &RemoteFile,
    dest: &Path,
    cancel: &AtomicBool,
    mut on_bytes: impl FnMut(u64),
) -> Result<(), LocalError> {
    let part = part_path(dest, &f.path);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > f.size {
        let _ = std::fs::remove_file(&part);
        have = 0;
    }

    // Hash what we already have so the final checksum covers the whole file.
    // (In chunks: a partial shard can be several gigabytes.)
    let mut hasher = Sha256::new();
    if have > 0 && f.sha256.is_some() {
        use tokio::io::AsyncReadExt;
        let mut existing = tokio::fs::File::open(&part)
            .await
            .map_err(LocalError::io("reading a partial download"))?;
        let mut buf = vec![0u8; 1 << 20];
        let mut left = have;
        while left > 0 {
            let n = existing
                .read(&mut buf)
                .await
                .map_err(LocalError::io("reading a partial download"))?;
            if n == 0 {
                break;
            }
            let n = n.min(left as usize);
            hasher.update(&buf[..n]);
            left -= n as u64;
        }
    }

    let url = hub.file_url(req.repo, req.revision, &f.path);
    let mut rq = hub.request(&url);
    if have > 0 {
        rq = rq.header("Range", format!("bytes={have}-"));
    }
    let resp = rq
        .send()
        .await
        .map_err(|_| LocalError::Network(hub.base().into()))?;
    let status = resp.status().as_u16();
    // A 206 must continue exactly where the partial file ends.
    if status == 206 {
        let starts_right = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| {
                v.trim_start_matches("bytes ")
                    .starts_with(&format!("{have}-"))
            });
        if !starts_right {
            let _ = std::fs::remove_file(&part);
            return Err(LocalError::Check(format!(
                "{} couldn't be resumed (the server sent a different part); try again",
                f.path
            )));
        }
    }
    let mut file = match status {
        206 => tokio::fs::OpenOptions::new().append(true).open(&part).await,
        200 => {
            // Server ignored the range: start over.
            have = 0;
            hasher = Sha256::new();
            tokio::fs::File::create(&part).await
        }
        401 | 403 => return Err(LocalError::NeedsAccess),
        416 if have == f.size => tokio::fs::OpenOptions::new().append(true).open(&part).await,
        _ => return Err(LocalError::Http { status, url }),
    }
    .map_err(LocalError::io("opening the download file"))?;

    if status != 416 {
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                let _ = file.flush().await;
                return Err(LocalError::Cancelled);
            }
            let chunk = chunk.map_err(|_| LocalError::Network(hub.base().into()))?;
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(LocalError::io("writing the model file"))?;
            have += chunk.len() as u64;
            if have > f.size {
                drop(file);
                let _ = std::fs::remove_file(&part);
                return Err(LocalError::Check(format!(
                    "{} is larger than expected, so it was discarded",
                    f.path
                )));
            }
            on_bytes(have);
        }
    }
    file.flush()
        .await
        .map_err(LocalError::io("writing the model file"))?;
    file.sync_all()
        .await
        .map_err(LocalError::io("writing the model file"))?;
    drop(file);

    if have != f.size {
        return Err(LocalError::Check(format!(
            "{} ended early ({have} of {} bytes); try again to resume",
            f.path, f.size
        )));
    }
    if let Some(expected) = &f.sha256 {
        let got = hex(&hasher.finalize());
        if !got.eq_ignore_ascii_case(expected) {
            let _ = std::fs::remove_file(&part);
            return Err(LocalError::Checksum {
                file: f.path.clone(),
            });
        }
    }
    std::fs::rename(&part, dest.join(&f.path)).map_err(LocalError::io("finishing the model file"))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A finished download found in the models folder.
#[derive(Debug, Clone, Serialize)]
pub struct Installed {
    pub dir_name: String,
    pub manifest: Manifest,
}

/// Lists completed downloads.
pub fn installed(models_dir: &Path) -> Vec<Installed> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(models_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let Ok(bytes) = std::fs::read(entry.path().join(MANIFEST)) else {
            continue;
        };
        if let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) {
            out.push(Installed {
                dir_name: entry.file_name().to_string_lossy().into_owned(),
                manifest,
            });
        }
    }
    out.sort_by(|a, b| a.manifest.repo.cmp(&b.manifest.repo));
    out
}

/// Deletes a model folder (complete or partial). `dir_name` must be a plain
/// folder name as produced by [`dir_name`].
pub fn remove(models_dir: &Path, dir_name: &str) -> Result<(), LocalError> {
    if dir_name.is_empty() || dir_name.contains(['/', '\\']) || dir_name.starts_with('.') {
        return Err(LocalError::Check("invalid model folder name".into()));
    }
    let path = models_dir.join(dir_name);
    if path.exists() {
        std::fs::remove_dir_all(&path).map_err(LocalError::io("deleting the model"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// A tiny file server that supports Range requests.
    async fn serve(files: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let files = files.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let name = path.rsplit('/').next().unwrap_or("");
                    let Some((_, body)) = files.iter().find(|(f, _)| *f == name) else {
                        let _ = sock
                            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                            .await;
                        return;
                    };
                    let start = req
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("range: bytes=")
                                .map(|r| r.trim_end_matches('-').parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    let (code, slice) = if start > 0 {
                        ("206 Partial Content", &body[start..])
                    } else {
                        ("200 OK", &body[..])
                    };
                    let head = format!(
                        "HTTP/1.1 {code}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        slice.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(slice).await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn sha(b: &[u8]) -> String {
        hex(&Sha256::digest(b))
    }

    #[tokio::test]
    async fn downloads_resumes_and_verifies() {
        let weights: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let config = b"{\"a\":1}".to_vec();
        let base = serve(vec![
            ("model.safetensors", weights.clone()),
            ("config.json", config.clone()),
        ])
        .await;
        let hub = Hub::with_base(&base, None);
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![
            RemoteFile {
                path: "config.json".into(),
                size: config.len() as u64,
                sha256: None,
            },
            RemoteFile {
                path: "model.safetensors".into(),
                size: weights.len() as u64,
                sha256: Some(sha(&weights)),
            },
        ];

        // Simulate an interrupted earlier attempt.
        let dest = tmp.path().join("org--m");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("model.safetensors.part"), &weights[..50_000]).unwrap();

        let req = Request {
            repo: "org/m",
            revision: "r1",
            parser: "hermes",
            files: &files,
        };
        let mut last = None;
        let out = download(&hub, req, tmp.path(), &AtomicBool::new(false), |p| {
            last = Some(p.clone())
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(out.join("model.safetensors")).unwrap(),
            weights
        );
        assert!(!out.join("model.safetensors.part").exists());
        assert_eq!(
            last.unwrap().done_bytes,
            (weights.len() + config.len()) as u64
        );

        let list = installed(tmp.path());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].manifest.parser, "hermes");
        remove(tmp.path(), "org--m").unwrap();
        assert!(installed(tmp.path()).is_empty());
        assert!(remove(tmp.path(), "../x").is_err());
    }

    #[tokio::test]
    async fn bad_checksum_is_rejected() {
        let base = serve(vec![("model.safetensors", b"tampered".to_vec())]).await;
        let hub = Hub::with_base(&base, None);
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![RemoteFile {
            path: "model.safetensors".into(),
            size: 8,
            sha256: Some(sha(b"original")),
        }];
        let req = Request {
            repo: "org/m",
            revision: "r",
            parser: "hermes",
            files: &files,
        };
        let err = download(&hub, req, tmp.path(), &AtomicBool::new(false), |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, LocalError::Checksum { .. }));
        assert!(installed(tmp.path()).is_empty());
    }

    #[tokio::test]
    async fn cancel_keeps_partial_file() {
        let big = vec![7u8; 2_000_000];
        let base = serve(vec![("model.safetensors", big.clone())]).await;
        let hub = Hub::with_base(&base, None);
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![RemoteFile {
            path: "model.safetensors".into(),
            size: big.len() as u64,
            sha256: None,
        }];
        let req = Request {
            repo: "org/m",
            revision: "r",
            parser: "hermes",
            files: &files,
        };
        let err = download(&hub, req, tmp.path(), &AtomicBool::new(true), |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, LocalError::Cancelled));
        assert!(installed(tmp.path()).is_empty());
    }
}
