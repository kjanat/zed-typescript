use crate::settings::{self, ExtensionSetting};
use zed_extension_api::{self as zed, LanguageServerId, Result};

pub const TYPESCRIPT_PACKAGE: &str = "typescript";

pub struct RequestedTypescriptSpec {
    pub install_spec: String,
    pub exact_version: Option<String>,
    pub include_prereleases: bool,
}

impl RequestedTypescriptSpec {
    pub fn uses_github(&self) -> bool {
        // Nightly/prerelease pins may exist only on npm.
        self.include_prereleases
            || self.install_spec == "latest"
            || self.exact_version.as_deref().is_some_and(|version| {
                semver::Version::parse(version).is_ok_and(|version| version.pre.is_empty())
            })
    }

    pub fn can_reuse_managed_version(&self, installed: &str) -> bool {
        !self.include_prereleases
            && (self.install_spec == "latest" || self.matches_installed(Some(installed)))
            && semver::Version::parse(installed)
                .is_ok_and(|version| version.major >= 7 && version.pre.is_empty())
    }

    fn matches_installed(&self, installed: Option<&str>) -> bool {
        self.exact_version.as_deref().is_some_and(|exact_version| {
            installed.is_some_and(|installed| installed == exact_version)
        })
    }
}

pub fn requested_typescript_spec(
    ext_settings: &Option<zed::serde_json::Value>,
) -> Result<RequestedTypescriptSpec> {
    if let Some(version) = settings::string_setting(ext_settings, ExtensionSetting::Version)? {
        let version = version.trim();
        if version.is_empty() {
            return Err("TypeScript version setting must not be empty".into());
        }
        return Ok(RequestedTypescriptSpec {
            install_spec: version.to_string(),
            exact_version: exact_version(version),
            include_prereleases: false,
        });
    }

    let Some(channel) = settings::string_setting(ext_settings, ExtensionSetting::UpdateChannel)?
    else {
        return Ok(latest_request());
    };

    match channel.as_str() {
        "latest" => Ok(latest_request()),
        "prerelease" => Ok(RequestedTypescriptSpec {
            include_prereleases: true,
            ..latest_request()
        }),
        "next" => Ok(RequestedTypescriptSpec {
            install_spec: "next".to_string(),
            exact_version: None,
            include_prereleases: false,
        }),
        _ => Err(format!(
            "unsupported TypeScript update channel `{channel}`; expected `latest`, `prerelease` or `next`"
        )),
    }
}

fn latest_request() -> RequestedTypescriptSpec {
    RequestedTypescriptSpec {
        install_spec: "latest".into(),
        exact_version: None,
        include_prereleases: false,
    }
}

/// Resolve npm's latest tag only when npm is the selected installation source.
pub fn npm_spec(requested: RequestedTypescriptSpec) -> Result<RequestedTypescriptSpec> {
    // A custom Node runtime needs npm's launcher. Resolve the GitHub channel
    // first so it still runs the selected release, rather than npm's latest tag.
    if requested.include_prereleases {
        let release = zed::latest_github_release(
            "microsoft/TypeScript",
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: true,
            },
        )
        .map(|release| release.version);
        return npm_prerelease_spec(release, std::path::Path::new(&managed_package_dir()?));
    }
    if requested.install_spec != "latest" {
        return Ok(requested);
    }
    match zed::npm_package_latest_version(TYPESCRIPT_PACKAGE) {
        Ok(latest) => {
            ensure_typescript_7_or_newer(&latest)?;
            Ok(RequestedTypescriptSpec {
                install_spec: latest.clone(),
                exact_version: Some(latest),
                include_prereleases: false,
            })
        }
        // registry unreachable (offline, proxy): reuse an existing managed 7+
        // install instead of failing the server start
        Err(error) => match zed::npm_package_installed_version(TYPESCRIPT_PACKAGE) {
            Ok(Some(installed)) if ensure_typescript_7_or_newer(&installed).is_ok() => {
                Ok(RequestedTypescriptSpec {
                    install_spec: installed.clone(),
                    exact_version: Some(installed),
                    include_prereleases: false,
                })
            }
            _ => Err(error),
        },
    }
}

const NPM_PRERELEASE_MARKER: &str = ".zed-github-prerelease";

fn remember_npm_prerelease(directory: &std::path::Path, version: &str) -> Result<()> {
    node_shim_path(&directory.to_string_lossy())?;
    std::fs::write(directory.join(NPM_PRERELEASE_MARKER), version)
        .map_err(|error| format!("failed to save the installed prerelease selection: {error}"))
}

fn installed_prerelease(directory: &std::path::Path) -> Option<String> {
    let selected = std::fs::read_to_string(directory.join(NPM_PRERELEASE_MARKER)).ok()?;
    let version = exact_version(selected.trim())?;
    ensure_typescript_7_or_newer(&version).ok()?;
    let metadata: zed::serde_json::Value =
        zed::serde_json::from_slice(&std::fs::read(directory.join("package.json")).ok()?).ok()?;
    (metadata["name"].as_str() == Some(TYPESCRIPT_PACKAGE)
        && metadata["version"].as_str() == Some(version.as_str())
        && directory.join("bin/tsc").is_file())
    .then_some(version)
}

fn npm_prerelease_spec(
    release: Result<String>,
    directory: &std::path::Path,
) -> Result<RequestedTypescriptSpec> {
    let version = match release {
        Ok(release) => exact_version(&release)
            .ok_or_else(|| "Invalid TypeScript version in GitHub release".to_string())?,
        // Only reuse a successful selection from this channel, with its exact
        // installed version and launcher still present after a restart.
        Err(error) => installed_prerelease(directory).ok_or(error)?,
    };
    ensure_typescript_7_or_newer(&version)?;
    Ok(RequestedTypescriptSpec {
        install_spec: version.clone(),
        exact_version: Some(version),
        include_prereleases: true,
    })
}

/// Installs the requested `typescript` package into the extension's working
/// directory and returns the installed package directory.
pub fn install_managed_typescript(
    language_server_id: &LanguageServerId,
    requested: &RequestedTypescriptSpec,
) -> Result<String> {
    let current = zed::npm_package_installed_version(TYPESCRIPT_PACKAGE)?;
    let is_tag = requested.exact_version.is_none();
    let needs_install = is_tag || !requested.matches_installed(current.as_deref());

    if needs_install {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::Downloading,
        );
        zed::npm_install_package(TYPESCRIPT_PACKAGE, &requested.install_spec)?;
    }

    let installed = zed::npm_package_installed_version(TYPESCRIPT_PACKAGE)?;
    let installed = installed
        .as_deref()
        .ok_or_else(|| "TypeScript was not installed after npm install completed".to_string())?;
    ensure_typescript_7_or_newer(installed)?;

    let directory = managed_package_dir()?;
    if requested.include_prereleases {
        if !requested.matches_installed(Some(installed)) {
            return Err("npm installed a different TypeScript version than requested".into());
        }
        remember_npm_prerelease(std::path::Path::new(&directory), installed)?;
    }
    Ok(directory)
}

pub fn managed_package_dir() -> Result<String> {
    let path = std::env::current_dir()
        .map_err(|error| format!("failed to read extension directory: {error}"))?
        .join("node_modules")
        .join(TYPESCRIPT_PACKAGE);

    Ok(path.to_string_lossy().into_owned().replace('\\', "/"))
}

/// Locates the native `tsc` executable that ships in the per-platform
/// `@typescript/typescript-<platform>-<arch>` package next to (or inside) the
/// managed `typescript` package inside the WASI work directory.
/// Returns `None` when the platform has no
/// prebuilt binary or the package layout is not recognized (pnpm virtual
/// stores, unusual hoisting) — callers fall back to running the package's
/// `bin/tsc` Node shim, which performs Node module resolution instead.
pub fn find_native_server_binary(
    package_dir: &str,
    platform: &crate::host_platform::Platform,
) -> Option<String> {
    let exe = platform.executable;
    let platform_package = format!("@typescript/typescript-{}", platform.name);
    let candidates = [
        // `package_dir` is itself a platform package (tsdk.path pointed straight at it)
        format!("{package_dir}/lib/{exe}"),
        // hoisted install: platform package is a sibling in the same node_modules
        format!("{package_dir}/../{platform_package}/lib/{exe}"),
        // nested install
        format!("{package_dir}/node_modules/{platform_package}/lib/{exe}"),
    ];

    candidates
        .into_iter()
        .find(|path| std::fs::metadata(path).is_ok_and(|m| m.is_file()))
}

pub fn node_shim_path(package_dir: &str) -> Result<String> {
    let shim = format!("{package_dir}/bin/tsc");
    if std::fs::metadata(&shim).is_ok_and(|m| m.is_file()) {
        Ok(shim)
    } else {
        Err(format!(
            "TypeScript package at `{package_dir}` has no native server binary for this platform and no `bin/tsc` launcher"
        ))
    }
}

pub fn ensure_typescript_7_or_newer(version: &str) -> Result<()> {
    let v = version.strip_prefix('v').unwrap_or(version);
    // take leading digits even if followed by - or . or pre
    let major_part = v.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("");
    if major_part.is_empty() {
        return Err(format!("invalid TypeScript version `{version}`"));
    }
    let major: u64 = major_part
        .parse()
        .map_err(|_| format!("invalid TypeScript version `{version}`"))?;
    if major < 7 {
        return Err(format!(
            "TypeScript LSP requires TypeScript 7 or newer, got `{version}`"
        ));
    }
    Ok(())
}

fn exact_version(version: &str) -> Option<String> {
    let v = version.trim().strip_prefix('v').unwrap_or(version.trim());
    semver::Version::parse(v)
        .ok()
        .map(|version| version.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_npm_fallback_respects_exact_pins_and_release_channels() {
        for pin in ["7.0.2", "v7.0.2"] {
            let request =
                requested_typescript_spec(&Some(zed::serde_json::json!({"version": pin}))).unwrap();
            assert!(request.can_reuse_managed_version("7.0.2"));
            for other in [
                "7.0.1",
                "7.0.3",
                "8.0.0",
                "7.0.2-beta.1",
                "6.0.2",
                "invalid",
            ] {
                assert!(!request.can_reuse_managed_version(other), "{pin}: {other}");
            }
        }
        for settings in [None, Some(zed::serde_json::json!({"version": "latest"}))] {
            let request = requested_typescript_spec(&settings).unwrap();
            assert!(request.can_reuse_managed_version("7.0.2"));
            assert!(request.can_reuse_managed_version("8.0.0"));
            assert!(!request.can_reuse_managed_version("7.1.0-beta.1"));
            assert!(!request.can_reuse_managed_version("6.0.2"));
        }
        for settings in [
            zed::serde_json::json!({"updateChannel": "prerelease"}),
            zed::serde_json::json!({"updateChannel": "next"}),
            zed::serde_json::json!({"version": "^7"}),
        ] {
            let request = requested_typescript_spec(&Some(settings)).unwrap();
            assert!(!request.can_reuse_managed_version("7.0.2"));
        }
    }

    #[test]
    fn npm_prerelease_offline_restart_requires_its_installed_selection() {
        let directory =
            std::env::temp_dir().join(format!("typescript-npm-prerelease-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("bin")).unwrap();
        for version in ["7.1.0-beta.1", "7.0.2"] {
            std::fs::write(
                directory.join("package.json"),
                zed::serde_json::json!({
                    "name": "typescript", "version": version,
                })
                .to_string(),
            )
            .unwrap();
            std::fs::write(directory.join("bin/tsc"), "launcher").unwrap();
            remember_npm_prerelease(&directory, version).unwrap();
            // No in-memory state survives: the installed selection is enough.
            let request = npm_prerelease_spec(Err("offline".into()), &directory).unwrap();
            assert_eq!(request.exact_version.as_deref(), Some(version));
            assert!(request.include_prereleases);
            assert!(request.matches_installed(Some(version)));
            // A newer online selection does not overwrite the successful one.
            let next = npm_prerelease_spec(Ok("v7.2.0-beta.1".into()), &directory).unwrap();
            assert_eq!(next.exact_version.as_deref(), Some("7.2.0-beta.1"));
            assert_eq!(installed_prerelease(&directory).as_deref(), Some(version));
            std::fs::remove_file(directory.join("bin/tsc")).unwrap();
            assert!(remember_npm_prerelease(&directory, "7.2.0-beta.1").is_err());
            assert!(npm_prerelease_spec(Err("offline".into()), &directory).is_err());
        }
        std::fs::write(directory.join("bin/tsc"), "launcher").unwrap();
        for marker in ["7.9.0-beta.1", "6.0.2", "garbage"] {
            std::fs::write(directory.join(NPM_PRERELEASE_MARKER), marker).unwrap();
            assert!(npm_prerelease_spec(Err("offline".into()), &directory).is_err());
        }
        std::fs::remove_file(directory.join(NPM_PRERELEASE_MARKER)).unwrap();
        assert!(npm_prerelease_spec(Err("offline".into()), &directory).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn test_ensure_7_or_newer() {
        assert!(ensure_typescript_7_or_newer("7.0.0").is_ok());
        assert!(ensure_typescript_7_or_newer("7.1.0-beta.1").is_ok());
        assert!(ensure_typescript_7_or_newer("10.0.0").is_ok());
        assert!(ensure_typescript_7_or_newer("v7.0").is_ok());
        assert!(ensure_typescript_7_or_newer("6.9.9").is_err());
        assert!(ensure_typescript_7_or_newer("foo").is_err());
    }

    #[test]
    fn test_exact_version() {
        assert_eq!(exact_version("7.0.2"), Some("7.0.2".into()));
        assert_eq!(exact_version("v7.0.2"), Some("7.0.2".into()));
        assert_eq!(exact_version("7.0.0-beta.1"), Some("7.0.0-beta.1".into()));
        assert_eq!(exact_version("latest"), None);
        assert_eq!(exact_version("next"), None);
        assert_eq!(exact_version("^7"), None);
        assert_eq!(exact_version("7"), None);
        assert_eq!(exact_version("7.0"), None);
        assert_eq!(exact_version("7.0.x"), None);
        assert_eq!(exact_version("7.0.0garbage"), None);
    }

    #[test]
    fn prerelease_channel_is_opt_in_and_version_still_wins() {
        let settings = Some(zed::serde_json::json!({"updateChannel": "prerelease"}));
        let requested = requested_typescript_spec(&settings).unwrap();
        assert!(requested.uses_github());
        assert!(requested.include_prereleases);
        assert!(requested.exact_version.is_none());
        assert!(
            !requested_typescript_spec(&None)
                .unwrap()
                .include_prereleases
        );

        for version in ["7.0.2", "next", "prerelease"] {
            let settings =
                Some(zed::serde_json::json!({"version": version, "updateChannel": "prerelease"}));
            let requested = requested_typescript_spec(&settings).unwrap();
            assert!(!requested.include_prereleases);
            assert_eq!(requested.install_spec, version);
        }
    }

    #[test]
    fn managed_source_preserves_npm_tags_and_ranges() {
        for (spec, github) in [
            ("7.0.2", true),
            ("v7.0.2", true),
            ("7.0.0-dev.20260912", false),
            ("latest", true),
            ("next", false),
            ("beta", false),
            ("7", false),
            ("7.0.x", false),
            ("^7.0.0", false),
            (">=6 <7 || >=7", false),
        ] {
            let settings = Some(zed::serde_json::json!({ "version": spec }));
            assert_eq!(
                requested_typescript_spec(&settings).unwrap().uses_github(),
                github,
                "{spec}"
            );
        }
        assert!(requested_typescript_spec(&None).unwrap().uses_github());
        let settings = Some(zed::serde_json::json!({"version": "next", "updateChannel": "latest"}));
        assert!(!requested_typescript_spec(&settings).unwrap().uses_github());
    }
}
