//! Copy a plugin package's `sop/` directory into the device SOP root.
//!
//! The package holds `sop/SOP.toml` and `sop/SOP.md`. The runtime loads
//! `<sops_dir>/<name>/`. This copies those two files under the procedure's
//! own name and checks that the engine can read them. An existing directory
//! is left alone.

use std::fs;
use std::path::{Path, PathBuf};

use super::SopExecutionMode;
use super::types::SopManifest;

const PROCEDURE_DIR: &str = "sop";
const PROCEDURE_TOML: &str = "SOP.toml";
const PROCEDURE_MARKDOWN: &str = "SOP.md";
const MAX_PROCEDURE_FILE_BYTES: u64 = 1024 * 1024;

/// A procedure copied, or already present, under the SOP root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSopInstall {
    /// Name from `[sop] name`. This is also the directory name.
    pub name: String,
    /// `<sops_dir>/<name>`.
    pub directory: PathBuf,
    /// The directory was already there, so nothing was written.
    pub already_present: bool,
}

/// Why a package procedure was not copied.
#[derive(Debug, thiserror::Error)]
pub enum PackageSopError {
    /// `[sop] name` is empty or would leave the SOP root.
    #[error("procedure name {0} must be a single path component")]
    BadName(String),
    /// `SOP.toml` is not the procedure manifest.
    #[error("{0}")]
    Manifest(String),
    /// `sop/` or one of its files is a symlink or another non-regular file.
    #[error("the procedure must be regular files inside the package")]
    Escapes,
    /// A procedure file is larger than the copy bound.
    #[error("{0} is larger than 1 MiB")]
    TooLarge(&'static str),
    /// The copied procedure is not one the engine can read.
    #[error("{0}")]
    Unreadable(String),
    /// A file could not be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Copy `package_dir/sop` into `sops_dir` when the package has a procedure.
///
/// `Ok(None)` means the package has no `sop/SOP.toml`. A procedure that fails
/// the read check is removed again, so a failed call leaves no new directory.
pub fn install_package_sop(
    package_dir: &Path,
    sops_dir: &Path,
) -> Result<Option<PackageSopInstall>, PackageSopError> {
    let sop_dir = package_dir.join(PROCEDURE_DIR);
    let toml_path = sop_dir.join(PROCEDURE_TOML);
    if !regular_file(&toml_path)? {
        return Ok(None);
    }
    if !real_directory(&sop_dir)? {
        return Err(PackageSopError::Escapes);
    }
    let manifest = read_manifest(&toml_path)?;
    let name = manifest.sop.name;
    let dest = super::resolve_sop_dir(sops_dir, &name)
        .map_err(|_| PackageSopError::BadName(name.clone()))?;

    let _lock = super::lock_sops_dir(sops_dir)
        .map_err(|error| PackageSopError::Unreadable(format!("{error:#}")))?;
    if fs::symlink_metadata(&dest).is_ok() {
        return Ok(Some(PackageSopInstall {
            name,
            directory: dest,
            already_present: true,
        }));
    }

    fs::create_dir_all(&dest)?;
    if let Err(error) = copy_and_read(&sop_dir, &toml_path, &dest) {
        let _ = fs::remove_dir_all(&dest);
        return Err(error);
    }
    Ok(Some(PackageSopInstall {
        name,
        directory: dest,
        already_present: false,
    }))
}

fn copy_and_read(sop_dir: &Path, toml_path: &Path, dest: &Path) -> Result<(), PackageSopError> {
    copy_bounded(toml_path, &dest.join(PROCEDURE_TOML), PROCEDURE_TOML)?;
    let markdown = sop_dir.join(PROCEDURE_MARKDOWN);
    match fs::symlink_metadata(&markdown) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(PackageSopError::Io(error)),
        Ok(meta) if meta.file_type().is_file() => {
            copy_bounded(
                &markdown,
                &dest.join(PROCEDURE_MARKDOWN),
                PROCEDURE_MARKDOWN,
            )?;
        }
        Ok(_) => return Err(PackageSopError::Escapes),
    }
    super::load_sop(dest, SopExecutionMode::Supervised)
        .map_err(|error| PackageSopError::Unreadable(format!("{error:#}")))?;
    Ok(())
}

fn read_manifest(toml_path: &Path) -> Result<SopManifest, PackageSopError> {
    let meta = fs::symlink_metadata(toml_path)?;
    if meta.len() > MAX_PROCEDURE_FILE_BYTES {
        return Err(PackageSopError::TooLarge(PROCEDURE_TOML));
    }
    let text = fs::read_to_string(toml_path)?;
    toml::from_str(&text).map_err(|error| PackageSopError::Manifest(error.to_string()))
}

fn copy_bounded(from: &Path, to: &Path, label: &'static str) -> Result<(), PackageSopError> {
    let meta = fs::symlink_metadata(from)?;
    if !meta.file_type().is_file() {
        return Err(PackageSopError::Escapes);
    }
    if meta.len() > MAX_PROCEDURE_FILE_BYTES {
        return Err(PackageSopError::TooLarge(label));
    }
    fs::copy(from, to)?;
    Ok(())
}

/// `Ok(true)` for a regular file. `Ok(false)` when the path is absent.
/// A symlink or other non-file is [`PackageSopError::Escapes`].
fn regular_file(path: &Path) -> Result<bool, PackageSopError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(PackageSopError::Io(error)),
        Ok(meta) if meta.file_type().is_file() => Ok(true),
        Ok(_) => Err(PackageSopError::Escapes),
    }
}

fn real_directory(path: &Path) -> Result<bool, PackageSopError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(PackageSopError::Io(error)),
        Ok(meta) if meta.file_type().is_dir() => Ok(true),
        Ok(_) => Err(PackageSopError::Escapes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROCEDURE: &str = r#"[sop]
name = "sample_pkg"
description = "Replace this. The starter step calls ping."
version = "0.1.0"

[[triggers]]
type = "manual"
"#;

    const STEPS: &str = r#"## Steps

1. **Ping** - Call the starter tool. Replace this step.
   - tools: ping
   - call: {"tool":"ping","args":{}}
"#;

    fn write_procedure(package: &Path) {
        let sop = package.join("sop");
        fs::create_dir_all(&sop).expect("sop dir");
        fs::write(sop.join("SOP.toml"), PROCEDURE).expect("toml");
        fs::write(sop.join("SOP.md"), STEPS).expect("md");
    }

    #[test]
    fn a_package_procedure_is_copied_under_its_own_name() {
        let root = tempfile::tempdir().expect("temp");
        let package = root.path().join("sample_pkg");
        let sops = root.path().join("sops");
        write_procedure(&package);

        let installed = install_package_sop(&package, &sops)
            .expect("install")
            .expect("present");
        assert_eq!(installed.name, "sample_pkg");
        assert!(!installed.already_present);
        let copied = fs::read_to_string(installed.directory.join("SOP.toml")).expect("copied");
        assert!(copied.contains("name = \"sample_pkg\""), "{copied}");
        assert!(
            fs::read_to_string(installed.directory.join("SOP.md"))
                .expect("steps")
                .contains("\"tool\":\"ping\"")
        );

        fs::write(
            package.join("sop").join("SOP.toml"),
            PROCEDURE.replace("ping", "other"),
        )
        .expect("edit");
        let again = install_package_sop(&package, &sops)
            .expect("second")
            .expect("present");
        assert!(again.already_present);
        let kept = fs::read_to_string(again.directory.join("SOP.toml")).expect("kept");
        assert!(kept.contains("ping"), "{kept}");
    }

    #[test]
    fn a_package_without_a_procedure_installs_nothing() {
        let root = tempfile::tempdir().expect("temp");
        let installed = install_package_sop(root.path(), &root.path().join("sops")).expect("none");
        assert!(installed.is_none());
        assert!(!root.path().join("sops").exists());
    }

    #[test]
    fn a_name_that_leaves_the_sop_root_is_refused() {
        let root = tempfile::tempdir().expect("temp");
        let package = root.path().join("pkg");
        write_procedure(&package);
        let toml_path = package.join("sop").join("SOP.toml");
        let text = fs::read_to_string(&toml_path).expect("read");
        fs::write(&toml_path, text.replace("sample_pkg", "../escape")).expect("rewrite");

        let error = install_package_sop(&package, &root.path().join("sops")).expect_err("name");
        assert!(matches!(error, PackageSopError::BadName(_)), "{error}");
        assert!(!root.path().join("escape").exists());
    }

    #[test]
    fn a_procedure_the_engine_cannot_read_is_removed() {
        let root = tempfile::tempdir().expect("temp");
        let package = root.path().join("pkg");
        write_procedure(&package);
        fs::write(
            package.join("sop").join("SOP.md"),
            "## Steps\n\n1. **Act**\n   - kind: capability\n   - capability: not_a_real_capability\n",
        )
        .expect("bad steps");

        let error = install_package_sop(&package, &root.path().join("sops")).expect_err("read");
        assert!(matches!(error, PackageSopError::Unreadable(_)), "{error}");
        assert!(!root.path().join("sops").join("sample_pkg").exists());
    }

    #[test]
    fn a_symlinked_procedure_is_refused() {
        let root = tempfile::tempdir().expect("temp");
        let package = root.path().join("pkg");
        let outside = root.path().join("outside");
        write_procedure(&outside);
        fs::create_dir_all(&package).expect("pkg");
        std::os::unix::fs::symlink(&outside.join("sop"), package.join("sop")).expect("link");

        let error = install_package_sop(&package, &root.path().join("sops")).expect_err("link");
        assert!(matches!(error, PackageSopError::Escapes), "{error}");
        assert!(!root.path().join("sops").exists());
    }
}
