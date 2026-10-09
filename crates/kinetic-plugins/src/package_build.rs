//! `kinetic build` compiles a plugin package with the Cargo already on this machine.
//!
//! The Kinetic binary does not contain a compiler. This module runs `cargo`
//! and `rustc`, copies the component to the manifest's `wasm_path`, and records
//! its SHA-256.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::signature;

const WASM_TARGET: &str = "wasm32-wasip2";

/// A component written beside its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltPackage {
    /// Path of the copied component.
    pub wasm: PathBuf,
    /// Lowercase SHA-256 of those bytes.
    pub sha256: String,
}

/// Why `kinetic build` stopped before or after Cargo.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The directory has no plugin crate.
    #[error("{} is not a plugin package", .0.display())]
    NotAPackage(PathBuf),
    /// `cargo` is not on PATH.
    #[error("cargo is not installed")]
    CargoMissing,
    /// The WebAssembly target is not installed.
    #[error("wasm32-wasip2 is not installed")]
    TargetMissing,
    /// Cargo exited non-zero. Its own message was already shown.
    #[error("the build failed")]
    Failed,
    /// `wasm_path` would leave the package directory.
    #[error("wasm_path {0} escapes the package")]
    BadWasmPath(String),
    /// `manifest.toml` or `Cargo.toml` could not be read as TOML.
    #[error("{0}")]
    Manifest(String),
    /// A file could not be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Compile `dir` and record the component hash in `manifest.toml`.
pub fn build_package(dir: &Path) -> Result<BuiltPackage, BuildError> {
    let cargo_toml = dir.join("Cargo.toml");
    let manifest_path = dir.join("manifest.toml");
    if !cargo_toml.is_file() || !manifest_path.is_file() {
        return Err(BuildError::NotAPackage(dir.to_path_buf()));
    }
    ensure_toolchain()?;
    let relative = wasm_path_from_manifest(&manifest_path)?;
    let destination = wasm_destination(dir, &relative)?;
    run_cargo(dir)?;
    let produced = produced_wasm(dir, &cargo_toml)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&produced, &destination)?;
    let bytes = fs::read(&destination)?;
    let sha256 = signature::sha256_hex(&bytes);
    write_wasm_digest(&manifest_path, &relative, &sha256)?;
    Ok(BuiltPackage {
        wasm: destination,
        sha256,
    })
}

fn ensure_toolchain() -> Result<(), BuildError> {
    match Command::new("cargo").arg("--version").output() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(BuildError::CargoMissing);
        }
        Err(error) => return Err(BuildError::Io(error)),
        Ok(output) if !output.status.success() => return Err(BuildError::CargoMissing),
        Ok(_) => {}
    }
    let output = match Command::new("rustc")
        .args(["--print", "target-libdir", "--target", WASM_TARGET])
        .output()
    {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(BuildError::CargoMissing);
        }
        Err(error) => return Err(BuildError::Io(error)),
        Ok(output) if !output.status.success() => return Err(BuildError::TargetMissing),
        Ok(output) => output,
    };
    let dir = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if !dir.is_dir() {
        return Err(BuildError::TargetMissing);
    }
    Ok(())
}

fn run_cargo(dir: &Path) -> Result<(), BuildError> {
    let status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--target")
        .arg(WASM_TARGET)
        .arg("--target-dir")
        .arg(dir.join("target"))
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(BuildError::Failed)
    }
}

fn wasm_path_from_manifest(manifest_path: &Path) -> Result<String, BuildError> {
    let text = fs::read_to_string(manifest_path)?;
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    let relative = doc
        .get("wasm_path")
        .and_then(|value| value.as_str())
        .unwrap_or("plugin.wasm");
    if !relative_wasm_path_ok(relative) {
        return Err(BuildError::BadWasmPath(relative.to_string()));
    }
    Ok(relative.to_string())
}

fn relative_wasm_path_ok(relative: &str) -> bool {
    let path = Path::new(relative);
    !relative.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, std::path::Component::ParentDir))
}

fn wasm_destination(dir: &Path, relative: &str) -> Result<PathBuf, BuildError> {
    Ok(dir.join(relative))
}

fn produced_wasm(dir: &Path, cargo_toml: &Path) -> Result<PathBuf, BuildError> {
    let name = wasm_artifact_name(cargo_toml)?;
    let path = dir
        .join("target")
        .join(WASM_TARGET)
        .join("debug")
        .join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(BuildError::Manifest(format!(
            "cargo finished but {} was not produced",
            path.display()
        )))
    }
}

fn wasm_artifact_name(cargo_toml: &Path) -> Result<String, BuildError> {
    let text = fs::read_to_string(cargo_toml)?;
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    let lib_name = doc
        .get("lib")
        .and_then(|lib| lib.get("name"))
        .and_then(|name| name.as_str());
    let package_name = doc
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str());
    let name = lib_name
        .or(package_name)
        .ok_or_else(|| BuildError::Manifest("Cargo.toml is missing package.name".to_string()))?;
    Ok(format!("{}.wasm", name.replace('-', "_")))
}

/// Set `wasm_path` and `wasm_sha256`, keeping the rest of the manifest.
pub(crate) fn write_wasm_digest(
    manifest_path: &Path,
    relative: &str,
    digest: &str,
) -> Result<(), BuildError> {
    let text = fs::read_to_string(manifest_path)?;
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    doc["wasm_path"] = toml_edit::value(relative);
    doc["wasm_sha256"] = toml_edit::value(digest);
    fs::write(manifest_path, doc.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_directory_without_a_crate_is_refused_before_cargo() {
        let root = tempfile::tempdir().expect("temp");
        let error = build_package(root.path()).expect_err("not a package");
        assert!(matches!(error, BuildError::NotAPackage(_)), "{error}");
    }

    #[test]
    fn the_digest_is_written_and_the_comment_stays() {
        let root = tempfile::tempdir().expect("temp");
        let manifest = root.path().join("manifest.toml");
        fs::write(
            &manifest,
            "# keep me\nname = \"sample_pkg\"\nwasm_path = \"plugin.wasm\"\n",
        )
        .expect("write");
        let digest = "ab".repeat(32);
        write_wasm_digest(&manifest, "plugin.wasm", &digest).expect("digest");
        let text = fs::read_to_string(&manifest).expect("read");
        assert!(text.contains("# keep me"), "{text}");
        assert!(
            text.contains(&format!("wasm_sha256 = \"{digest}\"")),
            "{text}"
        );
    }

    #[test]
    fn build_package_compiles_the_starter_and_records_the_digest() {
        let root = tempfile::tempdir().expect("temp");
        let path = root.path().join("sample_pkg");
        crate::scaffold::create_package(&path).expect("scaffold");
        let built = build_package(&path).expect("build");
        assert!(built.wasm.is_file(), "{}", built.wasm.display());
        assert_eq!(built.sha256.len(), 64);
        let bytes = fs::read(&built.wasm).expect("wasm");
        assert_eq!(signature::sha256_hex(&bytes), built.sha256);
        let manifest = fs::read_to_string(path.join("manifest.toml")).expect("manifest");
        assert!(manifest.contains(&built.sha256), "{manifest}");
    }
}
