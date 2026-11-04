use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use ureq::tls::{RootCerts, TlsConfig};

const REPOSITORY: &str = "Glubiz/workuum";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 4096;
const MAX_RELEASE_BYTES: u64 = 1024 * 1024;

fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text
        .strip_prefix('v')
        .unwrap_or(text)
        .split('.')
        .map(|part| part.parse().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

fn asset_name(os: &str, arch: &str, version: &str) -> Result<String, String> {
    let suffix = match (os, arch) {
        ("linux", "x86_64") => "linux-x86_64.tar.gz",
        ("linux", "aarch64") => "linux-aarch64.tar.gz",
        ("macos", _) => "macos.tar.gz",
        ("windows", _) => "windows.exe",
        _ => {
            return Err(format!(
                "no prebuilt workuum for {os}/{arch}; build it with: cargo install --git https://github.com/{REPOSITORY} workuum"
            ));
        }
    };
    Ok(format!("workuum-{version}-{suffix}"))
}

// The command of the package manager that owns this executable, if any.
fn package_manager_command(exe: &Path) -> Option<&'static [&'static str]> {
    let path = exe.to_string_lossy().to_lowercase().replace('\\', "/");
    if path.contains("/cellar/") || path.contains("/opt/homebrew/") || path.contains("linuxbrew") {
        return Some(&["brew", "upgrade", "workuum"]);
    }
    if path.contains("/chocolatey/lib/") {
        return Some(&["choco", "upgrade", "workuum", "-y"]);
    }
    None
}

fn verify_checksum(bytes: &[u8], text: &str, asset: &str) -> Result<(), String> {
    let mut fields = text.split_whitespace();
    let (Some(digest), Some(name), None) = (fields.next(), fields.next(), fields.next()) else {
        return Err(format!("malformed checksum file: '{}'", text.trim()));
    };
    // `sha256sum -b` prefixes the filename with `*`.
    let name = name.trim_start_matches('*');
    if name != asset {
        return Err(format!("checksum file names '{name}', expected '{asset}'"));
    }
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if !digest.eq_ignore_ascii_case(&actual) {
        return Err(format!(
            "checksum mismatch for {asset}: expected {digest}, got {actual}; not installing it"
        ));
    }
    Ok(())
}

fn extract_binary(archive: &[u8]) -> Result<Vec<u8>, String> {
    let invalid = |error: io::Error| format!("downloaded archive is invalid: {error}");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in archive.entries().map_err(invalid)? {
        let mut entry = entry.map_err(invalid)?;
        let path = entry.path().map_err(invalid)?.into_owned();
        if path != Path::new("workuum") && path != Path::new("./workuum") {
            continue;
        }
        let mut binary = Vec::new();
        (&mut entry)
            .take(MAX_DOWNLOAD_BYTES)
            .read_to_end(&mut binary)
            .map_err(invalid)?;
        return Ok(binary);
    }
    Err("downloaded archive does not contain workuum".into())
}

fn io_error(action: &str, path: &Path, error: io::Error) -> String {
    let hint = match (error.kind(), cfg!(windows)) {
        (io::ErrorKind::PermissionDenied, true) => "; run from an elevated shell",
        (io::ErrorKind::PermissionDenied, false) => "; re-run with sudo",
        _ => "",
    };
    format!("could not {action} {}: {error}{hint}", path.display())
}

// Windows cannot overwrite a running exe, but it can rename it out of the way.
fn replace(target: &Path, new: &Path) -> Result<(), String> {
    if cfg!(unix) {
        return fs::rename(new, target).map_err(|error| io_error("replace", target, error));
    }
    let mut old = target.with_extension("exe.old");
    // A previous image may still hold `.old` (a service that kept running), so park this one under a unique name.
    if fs::remove_file(&old).is_err() && old.exists() {
        old = target.with_extension(format!("exe.{}.old", std::process::id()));
    }
    fs::rename(target, &old).map_err(|error| io_error("rename", target, error))?;
    if let Err(error) = fs::rename(new, target) {
        let mut message = io_error("replace", target, error);
        if let Err(rollback) = fs::rename(&old, target) {
            message.push_str(&format!("; rollback failed: {rollback}"));
        }
        return Err(message);
    }
    // Fails while the old exe still runs; the next update removes it.
    let _ = fs::remove_file(&old);
    Ok(())
}

fn fetch(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        // The OS trust store, so TLS-inspecting endpoint security tools work.
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    agent
        .get(url)
        .header("User-Agent", concat!("workuum/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|error| format!("could not download {url}: {error}"))?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| format!("could not read {url}: {error}"))
}

pub fn run() -> Result<(), String> {
    let current = env!("CARGO_PKG_VERSION");
    let exe =
        std::env::current_exe().map_err(|error| format!("could not locate workuum: {error}"))?;
    let exe = dunce::canonicalize(&exe).map_err(|error| io_error("resolve", &exe, error))?;
    if let (true, Some(dir), Some(name)) = (
        cfg!(windows),
        exe.parent(),
        exe.file_name().and_then(|name| name.to_str()),
    ) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let file = entry.file_name();
            let file = file.to_string_lossy();
            if file.starts_with(name) && file.ends_with(".old") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    let release = fetch(
        &format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"),
        MAX_RELEASE_BYTES,
    )?;
    let release: Value = serde_json::from_slice(&release)
        .map_err(|error| format!("GitHub returned an unreadable latest release: {error}"))?;
    let tag = release["tag_name"].as_str().unwrap_or_default();
    let latest = parse_version(tag).ok_or(format!("unreadable release tag '{tag}'"))?;
    let installed = parse_version(current).ok_or(format!("unreadable version '{current}'"))?;
    if latest <= installed {
        println!("workuum {current} is up to date");
        return Ok(());
    }
    let version = tag.trim_start_matches('v');

    if let Some([program, args @ ..]) = package_manager_command(&exe) {
        let status = Command::new(program).args(args).status();
        if status.is_ok_and(|status| status.success()) {
            return Ok(());
        }
        return Err(format!(
            "`{program} {}` failed; run it manually to see why",
            args.join(" ")
        ));
    }

    let asset = asset_name(std::env::consts::OS, std::env::consts::ARCH, version)?;
    let url = format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{asset}");
    println!("downloading {url}");
    let downloaded = fetch(&url, MAX_DOWNLOAD_BYTES)?;
    let checksum = fetch(&format!("{url}.sha256"), MAX_CHECKSUM_BYTES)?;
    verify_checksum(&downloaded, &String::from_utf8_lossy(&checksum), &asset)?;
    let binary = if cfg!(windows) {
        downloaded
    } else {
        extract_binary(&downloaded)?
    };

    let new = exe.with_file_name(format!(
        ".workuum-update-{}{}",
        std::process::id(),
        std::env::consts::EXE_SUFFIX
    ));
    let result = (|| {
        fs::write(&new, &binary).map_err(|error| io_error("write", &new, error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&new, fs::Permissions::from_mode(0o755))
                .map_err(|error| io_error("set permissions on", &new, error))?;
        }
        let output = Command::new(&new)
            .arg("--version")
            .output()
            .map_err(|error| format!("could not run the downloaded binary: {error}"))?;
        let reported = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || !reported.contains(version) {
            return Err(format!(
                "downloaded binary did not report version {version}: {}",
                reported.trim()
            ));
        }
        replace(&exe, &new)
    })();
    let _ = fs::remove_file(&new);
    result?;
    println!("updated workuum {current} → {version}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    use super::*;

    const HELLO_SHA: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn archive(name: &str, content: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        builder.append_data(&mut header, name, content).unwrap();
        let mut encoder = builder.into_inner().unwrap();
        encoder.flush().unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn parses_versions_and_compares_them_numerically() {
        assert_eq!(parse_version("v0.10.2"), Some([0, 10, 2]));
        assert_eq!(parse_version("1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("1.2.x"), None);
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert!(parse_version("0.2.0") <= parse_version("0.2.0"));
    }

    #[test]
    fn picks_the_asset_for_each_supported_platform() {
        let name = |os, arch| asset_name(os, arch, "1.2.3").unwrap();
        assert_eq!(name("linux", "x86_64"), "workuum-1.2.3-linux-x86_64.tar.gz");
        assert_eq!(
            name("linux", "aarch64"),
            "workuum-1.2.3-linux-aarch64.tar.gz"
        );
        assert_eq!(name("macos", "aarch64"), "workuum-1.2.3-macos.tar.gz");
        assert_eq!(name("macos", "x86_64"), "workuum-1.2.3-macos.tar.gz");
        assert_eq!(name("windows", "aarch64"), "workuum-1.2.3-windows.exe");
    }

    #[test]
    fn unsupported_platforms_point_to_cargo_install() {
        for (os, arch) in [("linux", "riscv64"), ("freebsd", "x86_64")] {
            let error = asset_name(os, arch, "1.2.3").unwrap_err();
            assert!(
                error.contains("cargo install --git https://github.com/Glubiz/workuum workuum")
            );
        }
    }

    #[test]
    fn accepts_a_matching_checksum() {
        let text = format!("{HELLO_SHA}  workuum-1.0.0-macos.tar.gz\n");
        assert!(verify_checksum(b"hello", &text, "workuum-1.0.0-macos.tar.gz").is_ok());
        let binary_mode = format!("{} *workuum-1.0.0-macos.tar.gz", HELLO_SHA.to_uppercase());
        assert!(verify_checksum(b"hello", &binary_mode, "workuum-1.0.0-macos.tar.gz").is_ok());
    }

    #[test]
    fn rejects_a_checksum_mismatch() {
        let text = format!("{HELLO_SHA}  workuum-1.0.0-macos.tar.gz\n");
        let error = verify_checksum(b"hellO", &text, "workuum-1.0.0-macos.tar.gz").unwrap_err();
        assert!(error.contains("checksum mismatch"));
    }

    #[test]
    fn rejects_a_checksum_for_another_file() {
        let text = format!("{HELLO_SHA}  workuum-1.0.0-windows.exe\n");
        let error = verify_checksum(b"hello", &text, "workuum-1.0.0-macos.tar.gz").unwrap_err();
        assert!(error.contains("expected 'workuum-1.0.0-macos.tar.gz'"));
        assert!(verify_checksum(b"hello", "", "x").is_err());
        assert!(verify_checksum(b"hello", HELLO_SHA, "x").is_err());
    }

    #[test]
    fn classifies_the_package_manager_that_owns_the_exe() {
        let brew: &[&str] = &["brew", "upgrade", "workuum"];
        let choco: &[&str] = &["choco", "upgrade", "workuum", "-y"];
        for path in [
            "/usr/local/Cellar/workuum/0.2.0/bin/workuum",
            "/opt/homebrew/bin/workuum",
            "/home/linuxbrew/.linuxbrew/bin/workuum",
        ] {
            assert_eq!(
                package_manager_command(Path::new(path)),
                Some(brew),
                "{path}"
            );
        }
        for path in [
            r"C:\ProgramData\chocolatey\lib\workuum\tools\workuum.exe",
            "C:/ProgramData/chocolatey/lib/workuum/tools/workuum.exe",
        ] {
            assert_eq!(
                package_manager_command(Path::new(path)),
                Some(choco),
                "{path}"
            );
        }
        for path in ["/usr/local/bin/workuum", "/home/me/.cargo/bin/workuum"] {
            assert_eq!(package_manager_command(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn extracts_workuum_from_a_tarball() {
        assert_eq!(extract_binary(&archive("workuum", b"new")).unwrap(), b"new");
        assert_eq!(
            extract_binary(&archive("./workuum", b"new")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn rejects_a_tarball_without_workuum_and_garbage() {
        let error = extract_binary(&archive("other", b"x")).unwrap_err();
        assert!(error.contains("does not contain workuum"));
        assert!(extract_binary(b"not a tarball").is_err());
    }

    #[test]
    fn replace_swaps_the_content_and_consumes_the_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir
            .path()
            .join(format!("workuum{}", std::env::consts::EXE_SUFFIX));
        let new = dir.path().join("new");
        fs::write(&target, b"old").unwrap();
        fs::write(&new, b"new").unwrap();
        replace(&target, &new).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!new.exists());
    }

    #[cfg(windows)]
    #[test]
    fn replace_succeeds_when_a_previous_old_file_is_still_held() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("workuum.exe");
        let new = dir.path().join("new");
        fs::write(&target, b"old").unwrap();
        fs::write(&new, b"new").unwrap();
        // Share mode 0 stops the file being deleted or renamed over, like a running image.
        let _held = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(0)
            .open(dir.path().join("workuum.exe.old"))
            .unwrap();
        replace(&target, &new).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn replace_failure_leaves_the_new_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("new");
        fs::write(&new, b"new").unwrap();
        assert!(replace(&dir.path().join("nodir").join("workuum"), &new).is_err());
        assert_eq!(fs::read(&new).unwrap(), b"new");
    }
}
