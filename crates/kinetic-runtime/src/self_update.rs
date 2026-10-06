//! Replace the running binary with a published GitHub release.
//!
//! The download, checksum, and archive checks are the same ones `install.sh`
//! uses. Config, the device key, and `~/.kinetic` are not touched.

use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::time::Duration;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

use crate::i18n::{get_required_cli_string, get_required_cli_string_with_args};

const DEFAULT_REPO: &str = "tsmboa0/kinetic-vm";

/// What `kinetic update` found.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Current { version: String },
    Installed { version: String, path: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateError {
    Unsupported { os: String, arch: String },
    BadSource,
    Download,
    Checksum,
    Archive,
    Install(String),
}

impl UpdateError {
    pub fn message(&self) -> String {
        match self {
            Self::Unsupported { os, arch } => get_required_cli_string_with_args(
                "cli-update-unsupported",
                &[("os", os), ("arch", arch)],
            ),
            Self::BadSource => get_required_cli_string("cli-update-bad-source"),
            Self::Download => get_required_cli_string("cli-update-download"),
            Self::Checksum => get_required_cli_string("cli-update-checksum"),
            Self::Archive => get_required_cli_string("cli-update-archive"),
            Self::Install(path) => {
                get_required_cli_string_with_args("cli-update-install", &[("path", path)])
            }
        }
    }
}

/// Download the selected release and replace `current_exe`.
pub async fn run(force: bool) -> Result<Outcome, UpdateError> {
    let repo = repo_slug(&std::env::var("KINETIC_REPO").unwrap_or_default())?;
    let requested = requested_version(&std::env::var("KINETIC_VERSION").unwrap_or_default())?;
    let target = target_triple(std::env::consts::OS, std::env::consts::ARCH)?;
    let asset = archive_name(target);
    let client = http_client()?;
    let tag = if requested == "latest" {
        Box::pin(latest_tag(&client, &repo)).await?
    } else {
        requested.clone()
    };
    let current = env!("CARGO_PKG_VERSION");
    if !force && same_release(current, &tag) {
        return Ok(Outcome::Current {
            version: display_version(&tag),
        });
    }
    let (archive_url, sums_url) = download_urls(&repo, &tag, &asset);
    let archive = Box::pin(get_bytes(&client, &archive_url)).await?;
    let sums = Box::pin(get_text(&client, &sums_url)).await?;
    if !checksum_matches(&sums, &asset, &archive) {
        return Err(UpdateError::Checksum);
    }
    let binary = extract_binary(&archive)?;
    let dest = std::env::current_exe().map_err(|_| UpdateError::Install(String::new()))?;
    install_binary(&dest, &binary)?;
    Ok(Outcome::Installed {
        version: display_version(&tag),
        path: dest.display().to_string(),
    })
}

pub fn repo_slug(raw: &str) -> Result<String, UpdateError> {
    let repo = if raw.trim().is_empty() {
        DEFAULT_REPO
    } else {
        raw.trim()
    };
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(UpdateError::BadSource);
    };
    if owner.is_empty()
        || name.is_empty()
        || repo.split('/').nth(2).is_some()
        || !owner.chars().all(slug_char)
        || !name.chars().all(slug_char)
    {
        return Err(UpdateError::BadSource);
    }
    Ok(repo.to_string())
}

pub fn requested_version(raw: &str) -> Result<String, UpdateError> {
    let version = if raw.trim().is_empty() {
        "latest"
    } else {
        raw.trim()
    };
    if version.chars().all(slug_char) {
        Ok(version.to_string())
    } else {
        Err(UpdateError::BadSource)
    }
}

fn slug_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

/// Rust's `OS` and `ARCH` names, mapped to the published archive triple.
pub fn target_triple(os: &str, arch: &str) -> Result<&'static str, UpdateError> {
    let cpu = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => {
            return Err(UpdateError::Unsupported {
                os: os.to_string(),
                arch: arch.to_string(),
            });
        }
    };
    let sys = match os {
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        _ => {
            return Err(UpdateError::Unsupported {
                os: os.to_string(),
                arch: arch.to_string(),
            });
        }
    };
    match (cpu, sys) {
        ("x86_64", "unknown-linux-gnu") => Ok("x86_64-unknown-linux-gnu"),
        ("aarch64", "unknown-linux-gnu") => Ok("aarch64-unknown-linux-gnu"),
        ("x86_64", "apple-darwin") => Ok("x86_64-apple-darwin"),
        ("aarch64", "apple-darwin") => Ok("aarch64-apple-darwin"),
        _ => Err(UpdateError::Unsupported {
            os: os.to_string(),
            arch: arch.to_string(),
        }),
    }
}

pub fn archive_name(target: &str) -> String {
    format!("kinetic-{target}.tar.gz")
}

pub fn same_release(current: &str, tag: &str) -> bool {
    current.trim() == tag.trim().trim_start_matches('v')
}

fn display_version(tag: &str) -> String {
    let tag = tag.trim();
    if tag.starts_with('v') {
        tag.to_string()
    } else {
        format!("v{tag}")
    }
}

fn download_urls(repo: &str, tag: &str, asset: &str) -> (String, String) {
    let base = format!("https://github.com/{repo}/releases/download/{tag}");
    (format!("{base}/{asset}"), format!("{base}/SHA256SUMS"))
}

pub fn checksum_matches(sums: &str, asset: &str, bytes: &[u8]) -> bool {
    let Some(expected) = sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        if name == asset {
            Some(hash.to_ascii_lowercase())
        } else {
            None
        }
    }) else {
        return false;
    };
    expected.len() == 64
        && expected.chars().all(|ch| ch.is_ascii_hexdigit())
        && expected == hex::encode(Sha256::digest(bytes))
}

pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>, UpdateError> {
    let mut tar = tar::Archive::new(GzDecoder::new(Cursor::new(archive)));
    let entries = tar.entries().map_err(|_| UpdateError::Archive)?;
    let mut found = None;
    for entry in entries {
        let mut entry = entry.map_err(|_| UpdateError::Archive)?;
        let path = entry.path().map_err(|_| UpdateError::Archive)?.into_owned();
        if path != Path::new("kinetic") || found.is_some() {
            return Err(UpdateError::Archive);
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|_| UpdateError::Archive)?;
        if bytes.is_empty() {
            return Err(UpdateError::Archive);
        }
        found = Some(bytes);
    }
    found.ok_or(UpdateError::Archive)
}

pub fn install_binary(dest: &Path, bytes: &[u8]) -> Result<(), UpdateError> {
    let parent = dest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| UpdateError::Install(dest.display().to_string()))?;
    let tmp = parent.join(".kinetic-update");
    let wrote = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&tmp, dest)?;
        Ok(())
    })();
    if wrote.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err(UpdateError::Install(dest.display().to_string()));
    }
    Ok(())
}

fn http_client() -> Result<reqwest::Client, UpdateError> {
    reqwest::Client::builder()
        .user_agent("kinetic")
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| UpdateError::Download)
}

async fn latest_tag(client: &reqwest::Client, repo: &str) -> Result<String, UpdateError> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let release: Release = Box::pin(get_json(client, &url)).await?;
    if release.tag_name.is_empty() {
        return Err(UpdateError::Download);
    }
    Ok(release.tag_name)
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, UpdateError> {
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| UpdateError::Download)?;
    if !response.status().is_success() {
        return Err(UpdateError::Download);
    }
    response.json().await.map_err(|_| UpdateError::Download)
}

async fn get_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, UpdateError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| UpdateError::Download)?;
    if !response.status().is_success() {
        return Err(UpdateError::Download);
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|_| UpdateError::Download)
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String, UpdateError> {
    let bytes = Box::pin(get_bytes(client, url)).await?;
    String::from_utf8(bytes).map_err(|_| UpdateError::Download)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_triples_match_the_installer() {
        assert_eq!(
            target_triple("macos", "aarch64").expect("mac"),
            "aarch64-apple-darwin"
        );
        assert_eq!(
            target_triple("linux", "x86_64").expect("linux"),
            "x86_64-unknown-linux-gnu"
        );
        assert!(target_triple("windows", "x86_64").is_err());
        assert!(target_triple("linux", "arm").is_err());
    }

    #[test]
    fn a_release_tag_matches_the_crate_version() {
        assert!(same_release("0.8.5", "v0.8.5"));
        assert!(!same_release("0.8.5", "v0.8.6"));
    }

    #[test]
    fn a_bad_repo_is_refused() {
        assert_eq!(repo_slug("").expect("default"), DEFAULT_REPO);
        assert!(repo_slug("https://evil.example/x").is_err());
        assert!(repo_slug("owner/name/extra").is_err());
        assert!(requested_version("v0.8.6/../x").is_err());
    }

    #[test]
    fn the_checksum_accepts_the_archive_bytes_only() {
        let bytes = b"kinetic-bytes";
        let hash = hex::encode(Sha256::digest(bytes));
        let sums = format!("{hash}  kinetic-aarch64-apple-darwin.tar.gz\n");
        assert!(checksum_matches(
            &sums,
            "kinetic-aarch64-apple-darwin.tar.gz",
            bytes
        ));
        assert!(!checksum_matches(
            &sums,
            "kinetic-aarch64-apple-darwin.tar.gz",
            b"nope"
        ));
    }

    #[test]
    fn the_archive_must_contain_only_the_binary() {
        let archive = sample_archive("kinetic", b"#!/bin/kinetic");
        assert_eq!(extract_binary(&archive).expect("binary"), b"#!/bin/kinetic");
        let extra = sample_archive("README", b"nope");
        assert!(extract_binary(&extra).is_err());
    }

    #[test]
    fn install_replaces_the_destination() {
        let dir = tempfile::tempdir().expect("temp");
        let dest = dir.path().join("kinetic");
        std::fs::write(&dest, b"old").expect("seed");
        install_binary(&dest, b"new").expect("install");
        assert_eq!(std::fs::read(&dest).expect("read"), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        assert!(!dir.path().join(".kinetic-update").exists());
    }

    fn sample_archive(name: &str, contents: &[u8]) -> Vec<u8> {
        let mut raw = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut raw);
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, name, contents)
                .expect("tar");
            builder.finish().expect("finish");
        }
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&raw).expect("gzip");
        gzip.finish().expect("gzip finish")
    }
}
