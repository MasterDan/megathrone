use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const DEFAULT_SING_BOX_VERSION: &str = "1.14.0";
const SING_BOX_REPO: &str = "SagerNet/sing-box";

fn main() {
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=SING_BOX_VERSION");

    fetch_sing_box();
    tauri_build::build();
}

struct PlatformAsset {
    /// Release asset name without the archive extension, e.g. `sing-box-1.14.0-darwin-arm64`
    base_name: String,
    archive_ext: &'static str,
    is_windows: bool,
}

fn asset_for(target: &str, version: &str) -> PlatformAsset {
    let parts: Vec<&str> = target.split('-').collect();
    let arch = match parts[0] {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "armv7" | "arm" => "armv7",
        other => panic!("unsupported target arch `{other}` in triple `{target}`"),
    };

    let (os, archive_ext, is_windows) = if parts.contains(&"darwin") {
        ("darwin", "tar.gz", false)
    } else if parts.contains(&"linux") {
        ("linux", "tar.gz", false)
    } else if parts.contains(&"windows") {
        ("windows", "zip", true)
    } else {
        panic!("unsupported target OS in triple `{target}`");
    };

    // Plain linux assets are glibc builds; musl targets have a dedicated suffix.
    let abi_suffix = if os == "linux" && parts.contains(&"musl") {
        "-musl"
    } else {
        ""
    };

    PlatformAsset {
        base_name: format!("sing-box-{version}-{os}-{arch}{abi_suffix}"),
        archive_ext,
        is_windows,
    }
}

fn fetch_sing_box() {
    let target = env::var("TARGET").expect("cargo always sets TARGET");
    let version = env::var("SING_BOX_VERSION").unwrap_or_else(|_| DEFAULT_SING_BOX_VERSION.to_string());

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR");
    let bin_dir = Path::new(&manifest_dir).join("binaries");
    fs::create_dir_all(&bin_dir).expect("failed to create src-tauri/binaries");

    let asset = asset_for(&target, &version);
    let archive_name = format!("{}.{}", asset.base_name, asset.archive_ext);
    let exe_suffix = if asset.is_windows { ".exe" } else { "" };
    let dest = bin_dir.join(format!("sing-box-{target}{exe_suffix}"));
    let stamp = bin_dir.join(format!("sing-box-{target}{exe_suffix}.version"));

    let cached = dest.is_file() && fs::read_to_string(&stamp).map(|v| v == version).unwrap_or(false);
    if cached {
        println!("sing-box {version} for {target}: already fetched");
        return;
    }

    let url =
        format!("https://github.com/{SING_BOX_REPO}/releases/download/v{version}/{archive_name}");
    let tmp_dir = bin_dir.join(format!(".tmp-{target}"));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).expect("failed to create temp dir");
    let archive_path = tmp_dir.join(format!("archive.{}", asset.archive_ext));

    println!("sing-box {version} for {target}: downloading {url}");
    run(
        &mut Command::new("curl").args(["-fsSL", "-o"]).arg(&archive_path).arg(&url),
        "curl (download)",
    );

    let expected = expected_sha256(&archive_name, &version);
    verify_sha256(&archive_path, &expected);

    run(
        &mut Command::new("tar").arg("-xf").arg(&archive_path).arg("-C").arg(&tmp_dir),
        "tar (extract)",
    );

    let binary_name = if asset.is_windows { "sing-box.exe" } else { "sing-box" };
    let binary = find_file(&tmp_dir, binary_name)
        .unwrap_or_else(|| panic!("`{binary_name}` not found inside the extracted archive"));

    fs::copy(&binary, &dest).expect("failed to copy sing-box binary into place");
    if !asset.is_windows {
        make_executable(&dest);
    }
    fs::write(&stamp, &version).expect("failed to write version stamp");
    let _ = fs::remove_dir_all(&tmp_dir);

    println!("sing-box {version} for {target}: ready ({})", dest.display());
}

fn expected_sha256(asset_name: &str, version: &str) -> String {
    let api_url = format!("https://api.github.com/repos/{SING_BOX_REPO}/releases/tags/v{version}");
    let output = Command::new("curl")
        .args(["-fsSL", "-H", "Accept: application/vnd.github+json"])
        .arg(&api_url)
        .output()
        .expect("failed to spawn curl");
    if !output.status.success() {
        panic!(
            "failed to fetch release metadata ({api_url}): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON from GitHub API");
    json["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["name"].as_str() == Some(asset_name))
        .and_then(|entry| entry["digest"].as_str())
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .unwrap_or_else(|| panic!("no sha256 digest published for asset `{asset_name}`"))
        .to_lowercase()
}

fn verify_sha256(path: &Path, expected: &str) {
    let mut file = fs::File::open(path).unwrap_or_else(|e| panic!("failed to open archive: {e}"));
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).unwrap_or_else(|e| panic!("failed to read archive: {e}"));
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    assert_eq!(
        actual,
        expected.to_lowercase(),
        "sha256 mismatch for the downloaded sing-box archive"
    );
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|file_name| file_name == name) {
            return Some(path);
        }
    }
    None
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path).expect("failed to stat sing-box binary").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("failed to chmod sing-box binary");
}

fn run(command: &mut Command, what: &str) {
    let status = command.status().unwrap_or_else(|e| panic!("failed to spawn {what}: {e}"));
    if !status.success() {
        panic!("{what} exited with {status}");
    }
}
