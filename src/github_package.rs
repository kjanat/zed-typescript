use crate::host_platform::Platform;
use std::fs;
use std::path::{Path, PathBuf};
use zed_extension_api::{self as zed, LanguageServerId, Result};

const REPOSITORY: &str = "microsoft/TypeScript";

trait ReleaseHost {
    fn release(
        &mut self,
        version: Option<&str>,
        include_prereleases: bool,
    ) -> Result<zed::GithubRelease>;
    fn download(&mut self, url: &str, destination: &Path) -> Result<()>;
    fn executable(&mut self, path: &Path) -> Result<()>;
}

struct ZedHost<'a>(&'a LanguageServerId);

impl ReleaseHost for ZedHost<'_> {
    fn release(
        &mut self,
        version: Option<&str>,
        include_prereleases: bool,
    ) -> Result<zed::GithubRelease> {
        match version {
            Some(version) => zed::github_release_by_tag_name(REPOSITORY, &format!("v{version}")),
            None => zed::latest_github_release(
                REPOSITORY,
                zed::GithubReleaseOptions {
                    require_assets: true,
                    pre_release: include_prereleases,
                },
            ),
        }
    }

    fn download(&mut self, url: &str, destination: &Path) -> Result<()> {
        zed::set_language_server_installation_status(
            self.0,
            &zed::LanguageServerInstallationStatus::Downloading,
        );
        zed::download_file(
            url,
            &destination.to_string_lossy(),
            zed::DownloadedFileType::GzipTar,
        )
    }

    fn executable(&mut self, path: &Path) -> Result<()> {
        zed::make_file_executable(&path.to_string_lossy())
    }
}

pub fn install(
    language_server_id: &LanguageServerId,
    version: Option<&str>,
    include_prereleases: bool,
    platform: &Platform,
) -> Result<String> {
    let root = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .join("typescript-releases")
        .join(&platform.name);
    let directory = install_with(
        &root,
        platform,
        version,
        include_prereleases,
        &mut ZedHost(language_server_id),
    )?;
    Ok(directory.to_string_lossy().replace('\\', "/"))
}

fn release_version(value: &str) -> Result<String> {
    let version = semver::Version::parse(value.strip_prefix('v').unwrap_or(value))
        .map_err(|_| format!("Invalid TypeScript release version `{value}`"))?;
    if version.major < 7 {
        return Err(format!(
            "TypeScript LSP requires version 7 or newer, got `{version}`"
        ));
    }
    Ok(version.to_string())
}

fn valid_package(directory: &Path, platform: &Platform, version: &str) -> bool {
    let Ok(content) = fs::read(directory.join("package.json")) else {
        return false;
    };
    let Ok(metadata) = zed::serde_json::from_slice::<zed::serde_json::Value>(&content) else {
        return false;
    };
    metadata["name"].as_str() == Some(format!("@typescript/typescript-{}", platform.name).as_str())
        && metadata["version"].as_str() == Some(version)
        && [
            platform.executable,
            "lib.d.ts",
            "lib.es5.d.ts",
            "lib.dom.d.ts",
        ]
        .iter()
        .all(|file| directory.join("lib").join(file).is_file())
}

fn cached(root: &Path, platform: &Platform, version: &str) -> Option<PathBuf> {
    let directory = root.join(version);
    let package = directory.join("package");
    (directory.join("complete").is_file() && valid_package(&package, platform, version))
        .then_some(package)
}

fn install_with(
    root: &Path,
    platform: &Platform,
    requested: Option<&str>,
    include_prereleases: bool,
    host: &mut impl ReleaseHost,
) -> Result<PathBuf> {
    let marker = if include_prereleases {
        "prerelease"
    } else {
        "latest"
    };
    let requested = requested.map(release_version).transpose()?;
    if let Some(package) = requested.as_deref().and_then(|v| cached(root, platform, v)) {
        return Ok(package);
    }
    let release = match host.release(requested.as_deref(), include_prereleases) {
        Ok(release) => release,
        Err(error) => {
            // Each channel reuses only its own last successful selection.
            if requested.is_none()
                && let Some(package) = fs::read_to_string(root.join(marker))
                    .ok()
                    .and_then(|v| release_version(v.trim()).ok())
                    .and_then(|v| cached(root, platform, &v))
            {
                return Ok(package);
            }
            return Err(error);
        }
    };
    let version = release_version(&release.version)?;
    if requested
        .as_ref()
        .is_some_and(|requested| requested != &version)
    {
        return Err("GitHub returned a different TypeScript version than requested".into());
    }
    let package = match cached(root, platform, &version) {
        Some(package) => package,
        None => {
            let asset_name = format!("typescript-{}.tgz", platform.name);
            let asset = release
                .assets
                .iter()
                .find(|asset| asset.name == asset_name)
                .ok_or_else(|| {
                    format!("TypeScript {version} has no `{asset_name}` release asset")
                })?;
            let destination = root.join(&version);
            let partial = root.join(format!("{version}.partial"));
            if partial.exists() {
                fs::remove_dir_all(&partial).map_err(|e| e.to_string())?;
            }
            fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
            host.download(&asset.download_url, &partial)?;
            let package = partial.join("package");
            if !valid_package(&package, platform, &version) {
                return Err(format!(
                    "TypeScript {version} archive is missing its matching binary, metadata or standard libraries"
                ));
            }
            host.executable(&package.join("lib").join(platform.executable))?;
            fs::write(partial.join("complete"), "").map_err(|e| e.to_string())?;
            if destination.exists() {
                fs::remove_dir_all(&destination).map_err(|e| e.to_string())?;
            }
            fs::rename(&partial, &destination).map_err(|e| e.to_string())?;
            destination.join("package")
        }
    };
    if requested.is_none() {
        fs::write(root.join(marker), version).map_err(|e| e.to_string())?;
    }
    Ok(package)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static ID: AtomicU64 = AtomicU64::new(0);
            Self(std::env::temp_dir().join(format!(
                "typescript-release-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FakeHost {
        version: String,
        platform: Platform,
        offline: bool,
        missing_asset: bool,
        incomplete: bool,
        downloads: usize,
        lookups: usize,
        prerelease_requests: Vec<bool>,
        permissions: usize,
    }
    impl FakeHost {
        fn new() -> Self {
            Self {
                version: "7.0.2".into(),
                platform: Platform::new("linux", "x64").unwrap(),
                offline: false,
                missing_asset: false,
                incomplete: false,
                downloads: 0,
                lookups: 0,
                prerelease_requests: vec![],
                permissions: 0,
            }
        }
    }
    impl ReleaseHost for FakeHost {
        fn release(
            &mut self,
            _: Option<&str>,
            include_prereleases: bool,
        ) -> Result<zed::GithubRelease> {
            self.lookups += 1;
            self.prerelease_requests.push(include_prereleases);
            if self.offline {
                return Err("offline".into());
            }
            Ok(zed::GithubRelease {
                version: format!("v{}", self.version),
                assets: if self.missing_asset {
                    vec![]
                } else {
                    vec![zed::GithubReleaseAsset {
                        name: format!("typescript-{}.tgz", self.platform.name),
                        download_url: "https://github.com/test/asset".into(),
                    }]
                },
            })
        }
        fn download(&mut self, _: &str, destination: &Path) -> Result<()> {
            self.downloads += 1;
            let package = destination.join("package");
            fs::create_dir_all(package.join("lib")).unwrap();
            fs::write(package.join("package.json"), zed::serde_json::json!({
                "name": format!("@typescript/typescript-{}", self.platform.name), "version": self.version,
            }).to_string()).unwrap();
            for file in [
                self.platform.executable,
                "lib.d.ts",
                "lib.es5.d.ts",
                "lib.dom.d.ts",
            ] {
                if !(self.incomplete && file == "lib.dom.d.ts") {
                    fs::write(package.join("lib").join(file), "").unwrap();
                }
            }
            Ok(())
        }
        fn executable(&mut self, path: &Path) -> Result<()> {
            assert!(path.is_file());
            self.permissions += 1;
            Ok(())
        }
    }

    #[test]
    fn freebsd_release_selection_and_native_lookup_use_the_host_platform() {
        for arch in ["x64", "arm64"] {
            let root = Fixture::new();
            let mut host = FakeHost::new();
            host.platform = Platform::new("freebsd", arch).unwrap();
            let platform = Platform::new("freebsd", arch).unwrap();
            let package =
                install_with(&root.0, &platform, Some("7.0.2"), false, &mut host).unwrap();
            assert_eq!(host.downloads, 1);
            assert_eq!(host.permissions, 1);
            assert_eq!(
                crate::typescript_package::find_native_server_binary(
                    &package.to_string_lossy(),
                    &platform
                ),
                Some(format!("{}/lib/tsc", package.display())),
            );
        }
    }

    #[test]
    fn unlisted_host_architecture_reports_a_missing_asset() {
        let root = Fixture::new();
        let mut host = FakeHost::new();
        let platform = Platform::new("linux", "ia32").unwrap();
        let error = install_with(&root.0, &platform, None, false, &mut host).unwrap_err();
        assert!(error.contains("typescript-linux-ia32.tgz"));
        assert_eq!(host.downloads, 0);
    }

    #[test]
    fn exact_release_reuses_complete_install_without_network() {
        let root = Fixture::new();
        let mut host = FakeHost::new();
        let platform = Platform::new("linux", "x64").unwrap();
        let package = install_with(&root.0, &platform, Some("7.0.2"), false, &mut host).unwrap();
        host.offline = true;
        assert_eq!(
            install_with(&root.0, &platform, Some("v7.0.2"), false, &mut host).unwrap(),
            package
        );
        assert_eq!((host.lookups, host.downloads, host.permissions), (1, 1, 1));
    }

    #[test]
    fn latest_updates_and_reuses_last_success_offline() {
        let root = Fixture::new();
        let mut host = FakeHost::new();
        let platform = Platform::new("linux", "x64").unwrap();
        install_with(&root.0, &platform, None, false, &mut host).unwrap();
        host.version = "7.0.3".into();
        let package = install_with(&root.0, &platform, None, false, &mut host).unwrap();
        host.offline = true;
        assert_eq!(
            install_with(&root.0, &platform, None, false, &mut host).unwrap(),
            package
        );
        assert_eq!(host.downloads, 2);
        assert!(install_with(&root.0, &platform, Some("7.0.4"), false, &mut host).is_err());
    }

    #[test]
    fn partial_or_damaged_archives_are_retried_not_cached() {
        let root = Fixture::new();
        let mut host = FakeHost::new();
        let platform = Platform::new("linux", "x64").unwrap();
        host.incomplete = true;
        assert!(install_with(&root.0, &platform, None, false, &mut host).is_err());
        assert!(!root.0.join("latest").exists());
        host.incomplete = false;
        let package = install_with(&root.0, &platform, None, false, &mut host).unwrap();
        fs::remove_file(package.join("lib/tsc")).unwrap();
        install_with(&root.0, &platform, Some("7.0.2"), false, &mut host).unwrap();
        assert_eq!(host.downloads, 3);
    }

    #[test]
    fn prerelease_and_stable_channels_keep_separate_offline_selections() {
        let root = Fixture::new();
        let mut host = FakeHost::new();
        let platform = Platform::new("linux", "x64").unwrap();
        host.version = "7.1.0-beta.1".into();
        let preview = install_with(&root.0, &platform, None, true, &mut host).unwrap();
        host.offline = true;
        assert!(install_with(&root.0, &platform, None, false, &mut host).is_err());
        assert_eq!(
            install_with(&root.0, &platform, None, true, &mut host).unwrap(),
            preview
        );

        host.offline = false;
        host.version = "7.0.2".into();
        let stable = install_with(&root.0, &platform, None, false, &mut host).unwrap();
        host.offline = true;
        assert_eq!(
            install_with(&root.0, &platform, None, false, &mut host).unwrap(),
            stable
        );
        assert_eq!(
            install_with(&root.0, &platform, None, true, &mut host).unwrap(),
            preview
        );
        assert_eq!(
            host.prerelease_requests,
            [true, false, true, false, false, true]
        );
        assert_eq!(host.downloads, 2);
    }

    #[test]
    fn prerelease_channel_also_accepts_stable_releases() {
        let root = Fixture::new();
        let mut host = FakeHost::new();
        let platform = Platform::new("linux", "x64").unwrap();
        let package = install_with(&root.0, &platform, None, true, &mut host).unwrap();
        assert!(package.ends_with("7.0.2/package"));
        assert_eq!(
            fs::read_to_string(root.0.join("prerelease")).unwrap(),
            "7.0.2"
        );
        assert!(!root.0.join("latest").exists());
    }

    #[test]
    fn missing_assets_and_wrong_versions_fail_before_download() {
        let root = Fixture::new();
        let mut host = FakeHost::new();
        let platform = Platform::new("linux", "x64").unwrap();
        host.missing_asset = true;
        assert!(install_with(&root.0, &platform, None, false, &mut host).is_err());
        host.missing_asset = false;
        assert!(install_with(&root.0, &platform, Some("7.0.3"), false, &mut host).is_err());
        for invalid in ["6.0.2", "../outside", "7", "7.0.x"] {
            assert!(install_with(&root.0, &platform, Some(invalid), false, &mut host).is_err());
        }
        assert_eq!(host.downloads, 0);
    }
}
