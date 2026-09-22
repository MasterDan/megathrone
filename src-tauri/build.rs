use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const DEFAULT_SING_BOX_VERSION: &str = "1.14.0";
const SING_BOX_REPO: &str = "SagerNet/sing-box";

const DEFAULT_BYEDPI_VERSION: &str = "0.17.3";
const BYEDPI_REPO: &str = "hufrea/byedpi";
/// Release assets only ship Linux/Windows builds; on macOS the sidecar is
/// compiled from the source tarball (plain C, `make` + the system cc).
/// Digests are pinned per version; `BYEDPI_SOURCE_SHA256` overrides (for
/// `BYEDPI_VERSION` overrides).
const BYEDPI_SOURCE_SHA256: &str =
    "0a9cb8585554c68c3e2be88c33c9bf6f99f8e8c7f54b362285adab99e262566c";

fn main() {
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=PROFILE");
    println!("cargo:rerun-if-env-changed=SING_BOX_VERSION");
    println!("cargo:rerun-if-env-changed=BYEDPI_VERSION");

    let target = env::var("TARGET").expect("cargo always sets TARGET");
    let sing_box_version = fetch_sing_box(&target);
    let byedpi_version = fetch_byedpi(&target);
    if target.contains("android") {
        // The Android build does not use externalBin sidecars (the CLI never
        // bundles them into an APK): both binaries ship as fake `lib*.so`
        // jniLibs — the only files Android extracts with the exec bit — and
        // are exec'd by path from the APK's nativeLibraryDir at runtime.
        stage_android_jnilibs(&target, &sing_box_version, &byedpi_version);
    }
    if target.contains("-darwin") && env::var("PROFILE").as_deref() == Ok("release") {
        // `tauri build --target universal-apple-darwin` expects fat sidecars named
        // after the universal triple (`<name>-universal-apple-darwin`): the CLI
        // lipo-merges only the app binary, the sidecars must already be universal.
        // Fetch the other arch's slices and stitch both into one file.
        let other = other_darwin_triple(&target);
        fetch_sing_box(other);
        fetch_byedpi(other);
        lipo_universal("sing-box", &target, other, &sing_box_version);
        lipo_universal("byedpi", &target, other, &byedpi_version);
    }
    tauri_build::build();
}

fn other_darwin_triple(target: &str) -> &'static str {
    if target.starts_with("aarch64") {
        "x86_64-apple-darwin"
    } else {
        "aarch64-apple-darwin"
    }
}

fn lipo_universal(name: &str, triple_a: &str, triple_b: &str, version: &str) {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR");
    let bin_dir = Path::new(&manifest_dir).join("binaries");
    let dest = bin_dir.join(format!("{name}-universal-apple-darwin"));
    let stamp = bin_dir.join(format!("{name}-universal-apple-darwin.version"));

    let cached = dest.is_file() && fs::read_to_string(&stamp).map(|v| v == version).unwrap_or(false);
    if cached {
        println!("{name} {version} universal: already stitched");
        return;
    }

    let mut lipo = Command::new("lipo");
    lipo
        .arg("-create")
        .arg("-output")
        .arg(&dest)
        .arg(bin_dir.join(format!("{name}-{triple_a}")))
        .arg(bin_dir.join(format!("{name}-{triple_b}")));
    run(lipo, "lipo (stitch universal sidecar)");
    make_executable(&dest);
    fs::write(&stamp, version).expect("failed to write version stamp");

    println!("{name} {version} universal: ready ({})", dest.display());
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
        "i686" => "386",
        other => panic!("unsupported target arch `{other}` in triple `{target}`"),
    };

    // sing-box names its Android assets android-arm64/arm/amd64/386 (no
    // glibc/musl split — they are static bionic builds)
    let android_arch = match parts[0] {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "armv7" | "arm" => "arm",
        "i686" => "386",
        other => panic!("unsupported target arch `{other}` in triple `{target}`"),
    };

    let (os, archive_ext, is_windows, asset_arch) = if parts.contains(&"darwin") {
        ("darwin", "tar.gz", false, arch)
    } else if parts.contains(&"android") {
        ("android", "tar.gz", false, android_arch)
    } else if parts.contains(&"linux") {
        ("linux", "tar.gz", false, arch)
    } else if parts.contains(&"windows") {
        ("windows", "zip", true, arch)
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
        base_name: format!("sing-box-{version}-{os}-{asset_arch}{abi_suffix}"),
        archive_ext,
        is_windows,
    }
}

fn fetch_sing_box(target: &str) -> String {
    let version = env::var("SING_BOX_VERSION").unwrap_or_else(|_| DEFAULT_SING_BOX_VERSION.to_string());

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR");
    let bin_dir = Path::new(&manifest_dir).join("binaries");
    fs::create_dir_all(&bin_dir).expect("failed to create src-tauri/binaries");

    let asset = asset_for(target, &version);
    let archive_name = format!("{}.{}", asset.base_name, asset.archive_ext);
    let exe_suffix = if asset.is_windows { ".exe" } else { "" };
    let dest = bin_dir.join(format!("sing-box-{target}{exe_suffix}"));
    let stamp = bin_dir.join(format!("sing-box-{target}{exe_suffix}.version"));

    let cached = dest.is_file() && fs::read_to_string(&stamp).map(|v| v == version).unwrap_or(false);
    if cached {
        println!("sing-box {version} for {target}: already fetched");
        return version;
    }

    let url =
        format!("https://github.com/{SING_BOX_REPO}/releases/download/v{version}/{archive_name}");
    let tmp_dir = bin_dir.join(format!(".tmp-{target}"));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).expect("failed to create temp dir");
    let archive_path = tmp_dir.join(format!("archive.{}", asset.archive_ext));

    println!("sing-box {version} for {target}: downloading {url}");
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "-o"]).arg(&archive_path).arg(&url);
    run(curl, "curl (download)");

    let expected = expected_sha256(SING_BOX_REPO, &archive_name, &version);
    verify_sha256(&archive_path, &expected);

    let mut tar = Command::new("tar");
    tar.arg("-xf").arg(&archive_path).arg("-C").arg(&tmp_dir);
    run(tar, "tar (extract)");

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
    version
}

// ---------------------------------------------------------------------------
// byedpi (ciadpi)
// ---------------------------------------------------------------------------

/// Release asset names use the version without the leading `0.` ("0.17.3" →
/// "17.3").
fn byedpi_asset_version(version: &str) -> String {
    version.strip_prefix("0.").unwrap_or(version).to_string()
}

enum ByedpiAsset {
    /// Release asset base name (without extension) + archive type.
    Release { base_name: String, zip: bool },
    /// No prebuilt binaries for this platform — compile from sources.
    Source,
}

fn byedpi_asset_for(target: &str, asset_version: &str) -> ByedpiAsset {
    let parts: Vec<&str> = target.split('-').collect();
    let os = if parts.contains(&"darwin") {
        "darwin"
    } else if parts.contains(&"android") {
        // no prebuilt bionic binaries: compiled from source with the NDK
        // clang in the Source branch below
        return ByedpiAsset::Source;
    } else if parts.contains(&"linux") {
        "linux"
    } else if parts.contains(&"windows") {
        "windows"
    } else {
        panic!("unsupported target OS in triple `{target}`");
    };
    let arch = match parts[0] {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "armv7" | "arm" => "armv7l",
        "x86" | "i686" => "i686",
        other => panic!("unsupported target arch `{other}` in triple `{target}`"),
    };

    match (os, arch) {
        // upstream publishes static Linux ELF builds per arch…
        ("linux", _) => ByedpiAsset::Release {
            base_name: format!("byedpi-{asset_version}-{arch}"),
            zip: false,
        },
        // …and Windows zip archives for the two x86 flavors only
        ("windows", "x86_64" | "i686") => ByedpiAsset::Release {
            base_name: format!("byedpi-{asset_version}-{arch}-w64"),
            zip: true,
        },
        // …nothing for macOS — build from the source tarball
        ("darwin", "x86_64" | "aarch64") => ByedpiAsset::Source,
        (os, arch) => panic!("no byedpi build available for {os}/{arch}"),
    }
}

fn fetch_byedpi(target: &str) -> String {
    let version = env::var("BYEDPI_VERSION").unwrap_or_else(|_| DEFAULT_BYEDPI_VERSION.to_string());

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR");
    let bin_dir = Path::new(&manifest_dir).join("binaries");
    fs::create_dir_all(&bin_dir).expect("failed to create src-tauri/binaries");

    let asset = byedpi_asset_for(target, &byedpi_asset_version(&version));
    let is_windows = matches!(asset, ByedpiAsset::Release { zip: true, .. }) && target.contains("windows");
    let exe_suffix = if is_windows { ".exe" } else { "" };
    let dest = bin_dir.join(format!("byedpi-{target}{exe_suffix}"));
    let stamp = bin_dir.join(format!("byedpi-{target}{exe_suffix}.version"));

    let cached = dest.is_file() && fs::read_to_string(&stamp).map(|v| v == version).unwrap_or(false);
    if cached {
        println!("byedpi {version} for {target}: already fetched");
        return version;
    }

    let tmp_dir = bin_dir.join(format!(".tmp-byedpi-{target}"));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).expect("failed to create temp dir");

    match asset {
        ByedpiAsset::Release { base_name, zip } => {
            let archive_ext = if zip { "zip" } else { "tar.gz" };
            let archive_name = format!("{base_name}.{archive_ext}");
            let url =
                format!("https://github.com/{BYEDPI_REPO}/releases/download/v{version}/{archive_name}");

            println!("byedpi {version} for {target}: downloading {url}");
            let archive_path = tmp_dir.join(format!("archive.{archive_ext}"));
            let mut curl = Command::new("curl");
            curl.args(["-fsSL", "-o"]).arg(&archive_path).arg(&url);
            run(curl, "curl (download)");

            let expected = expected_sha256(BYEDPI_REPO, &archive_name, &version);
            verify_sha256(&archive_path, &expected);

            let mut tar = Command::new("tar");
            tar.arg("-xf").arg(&archive_path).arg("-C").arg(&tmp_dir);
            run(tar, "tar (extract)");

            // Prebuilt archives carry arch-suffixed binary names (`ciadpi-x86_64`),
            // the source build produces a bare `ciadpi` — accept both.
            let binary = find_byedpi_binary(&tmp_dir, is_windows)
                .unwrap_or_else(|| panic!("no ciadpi binary found inside the extracted archive"));
            fs::copy(&binary, &dest).expect("failed to copy byedpi binary into place");
        }
        ByedpiAsset::Source => {
            let url = format!(
                "https://github.com/{BYEDPI_REPO}/archive/refs/tags/v{version}.tar.gz"
            );
            println!("byedpi {version} for {target}: no prebuilt binary, building from source");
            let archive_path = tmp_dir.join("archive.tar.gz");
            let mut curl = Command::new("curl");
            curl.args(["-fsSL", "-o"]).arg(&archive_path).arg(&url);
            run(curl, "curl (download)");

            let expected = env::var("BYEDPI_SOURCE_SHA256")
                .unwrap_or_else(|_| BYEDPI_SOURCE_SHA256.to_string())
                .to_lowercase();
            verify_sha256(&archive_path, &expected);

            let src_dir = tmp_dir.join("src");
            fs::create_dir_all(&src_dir).expect("failed to create source dir");
            let mut tar = Command::new("tar");
            tar.arg("-xzf")
                .arg(&archive_path)
                .arg("-C")
                .arg(&src_dir)
                .arg("--strip-components")
                .arg("1");
            run(tar, "tar (extract)");

            // The compiler is picked per target: AppleClang builds for the
            // host arch by default, so the slice's arch is pinned (passed as
            // a make argument — overrides any makefile assignment, unlike an
            // env var) to make the x86_64 half of a universal build a real
            // cross-compile; Android compiles with the NDK's target-prefixed
            // clang wrapper (bionic).
            let cc = if target.contains("android") {
                ndk_clang(target)
            } else {
                let arch = if target.starts_with("aarch64") { "arm64" } else { "x86_64" };
                format!("cc -arch {arch}")
            };
            let mut make = Command::new("make");
            make.arg("-C").arg(&src_dir).arg(format!("CC={cc}"));
            run(make, "make (build ciadpi)");

            let binary = src_dir.join("ciadpi");
            assert!(binary.is_file(), "ciadpi was not produced by the build");
            fs::copy(&binary, &dest).expect("failed to copy byedpi binary into place");
        }
    }

    if !is_windows {
        make_executable(&dest);
    }
    fs::write(&stamp, &version).expect("failed to write version stamp");
    let _ = fs::remove_dir_all(&tmp_dir);

    println!("byedpi {version} for {target}: ready ({})", dest.display());
    version
}

// ---------------------------------------------------------------------------
// Android: stage the sidecars as fake jniLibs
// ---------------------------------------------------------------------------

/// Maps a cargo android triple to the Android ABI directory name.
fn android_abi(target: &str) -> &'static str {
    if target.starts_with("aarch64") {
        "arm64-v8a"
    } else if target.starts_with("armv7") {
        "armeabi-v7a"
    } else if target.starts_with("i686") {
        "x86"
    } else if target.starts_with("x86_64") {
        "x86_64"
    } else {
        panic!("unsupported android triple `{target}`")
    }
}

/// Copies the fetched sidecar binaries into the generated Gradle project as
/// `lib*.so` jniLibs — the naming Android's installer requires to extract
/// them into the APK's nativeLibraryDir with the exec bit (exec from the app
/// data dir is forbidden on modern Android). The gradle `rust` plugin merges
/// this directory when assembling every ABI flavor.
fn stage_android_jnilibs(target: &str, sing_box_version: &str, byedpi_version: &str) {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR");
    let bin_dir = Path::new(&manifest_dir).join("binaries");
    let jni_dir = Path::new(&manifest_dir)
        .join("gen/android/app/src/main/jniLibs")
        .join(android_abi(target));
    if !Path::new(&manifest_dir).join("gen/android").is_dir() {
        panic!(
            "the android gradle project is missing — run `pnpm tauri android init` \
             before building for an android target"
        );
    }
    fs::create_dir_all(&jni_dir).expect("failed to create the jniLibs directory");

    let copies = [
        (
            bin_dir.join(format!("sing-box-{target}")),
            jni_dir.join("libsingbox.so"),
            "sing-box",
            sing_box_version,
        ),
        (
            bin_dir.join(format!("byedpi-{target}")),
            jni_dir.join("libciadpi.so"),
            "byedpi",
            byedpi_version,
        ),
    ];
    for (src, dest, name, version) in copies {
        if !src.is_file() {
            panic!("{name} {version} for {target}: expected binary at {}", src.display());
        }
        fs::copy(&src, &dest)
            .unwrap_or_else(|e| panic!("failed to stage {} as {}: {e}", src.display(), dest.display()));
    }
    println!(
        "android sidecars for {target}: staged into {} (libsingbox.so, libciadpi.so)",
        jni_dir.display()
    );
}

/// Locates the NDK's target-prefixed clang wrapper for the given android
/// triple (min API 24, matching the gradle minSdk). NDK root comes from
/// `NDK_HOME`/`ANDROID_NDK_HOME`/`ANDROID_NDK_ROOT` — the same variables the
/// Tauri CLI uses.
fn ndk_clang(target: &str) -> String {
    let ndk = ["NDK_HOME", "ANDROID_NDK_HOME", "ANDROID_NDK_ROOT"]
        .iter()
        .find_map(env::var_os)
        .unwrap_or_else(|| {
            panic!(
                "building byedpi for android needs the NDK — set NDK_HOME (or \
                 ANDROID_NDK_HOME/ANDROID_NDK_ROOT) to the NDK directory"
            )
        });
    let prebuilt = Path::new(&ndk).join("toolchains/llvm/prebuilt");
    let host = fs::read_dir(&prebuilt)
        .unwrap_or_else(|e| panic!("failed to list {}: {e}", prebuilt.display()))
        .flatten()
        .find(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .unwrap_or_else(|| panic!("no host toolchain directory inside {}", prebuilt.display()));
    let prefix = match target.split('-').next().unwrap_or_default() {
        "aarch64" => "aarch64-linux-android",
        "armv7" => "armv7a-linux-androideabi",
        "i686" => "i686-linux-android",
        "x86_64" => "x86_64-linux-android",
        other => panic!("unsupported android arch `{other}` in triple `{target}`"),
    };
    let clang = host.join("bin").join(format!("{prefix}24-clang"));
    if !clang.is_file() {
        panic!("NDK clang not found at {}", clang.display());
    }
    clang.to_string_lossy().to_string()
}

fn expected_sha256(repo: &str, asset_name: &str, version: &str) -> String {
    let api_url = format!("https://api.github.com/repos/{repo}/releases/tags/v{version}");
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "-H", "Accept: application/vnd.github+json"]);
    // Anonymous API calls from CI runners share a 60 req/h per-IP limit;
    // ride the workflow token when one is present.
    if let Ok(token) = env::var("GITHUB_TOKEN").or_else(|_| env::var("GH_TOKEN")) {
        curl.args(["-H", &format!("Authorization: Bearer {token}")]);
    }
    let output = curl
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

fn find_byedpi_binary(dir: &Path, is_windows: bool) -> Option<PathBuf> {
    let exact = if is_windows { "ciadpi.exe" } else { "ciadpi" };
    find_file(dir, exact).or_else(|| find_file_prefixed(dir, "ciadpi"))
}

fn find_file_prefixed(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_prefixed(&path, prefix) {
                return Some(found);
            }
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix))
        {
            return Some(path);
        }
    }
    None
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions =
        fs::metadata(path).unwrap_or_else(|e| panic!("failed to stat {}: {e}", path.display()))
            .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .unwrap_or_else(|e| panic!("failed to chmod {}: {e}", path.display()));
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn run(mut command: Command, what: &str) {
    let status = command.status().unwrap_or_else(|e| panic!("failed to spawn {what}: {e}"));
    if !status.success() {
        panic!("{what} exited with {status}");
    }
}
