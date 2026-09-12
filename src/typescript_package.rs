use crate::settings::{self, ExtensionSetting};
use zed_extension_api::{self as zed, LanguageServerId, Result};

pub const TYPESCRIPT_PACKAGE: &str = "typescript";

pub struct RequestedTypescriptSpec {
    pub install_spec: String,
    pub exact_version: Option<String>,
}

impl RequestedTypescriptSpec {
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
        });
    }

    let Some(channel) = settings::string_setting(ext_settings, ExtensionSetting::UpdateChannel)?
    else {
        return latest_stable_spec();
    };

    match channel.as_str() {
        "latest" => latest_stable_spec(),
        "next" => Ok(RequestedTypescriptSpec {
            install_spec: "next".to_string(),
            exact_version: None,
        }),
        _ => Err(format!(
            "unsupported TypeScript update channel `{channel}`; expected `latest` or `next`"
        )),
    }
}

fn latest_stable_spec() -> Result<RequestedTypescriptSpec> {
    match zed::npm_package_latest_version(TYPESCRIPT_PACKAGE) {
        Ok(latest) => {
            ensure_typescript_7_or_newer(&latest)?;
            Ok(RequestedTypescriptSpec {
                install_spec: latest.clone(),
                exact_version: Some(latest),
            })
        }
        // registry unreachable (offline, proxy): reuse an existing managed 7+
        // install instead of failing the server start
        Err(error) => match zed::npm_package_installed_version(TYPESCRIPT_PACKAGE) {
            Ok(Some(installed)) if ensure_typescript_7_or_newer(&installed).is_ok() => {
                Ok(RequestedTypescriptSpec {
                    install_spec: installed.clone(),
                    exact_version: Some(installed),
                })
            }
            _ => Err(error),
        },
    }
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

    managed_package_dir()
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
    let v = version.strip_prefix('v').unwrap_or(version).trim();
    if v.is_empty() {
        return None;
    }
    if !v.chars().next().unwrap_or(' ').is_ascii_digit() {
        return None;
    }
    let ok = v
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c.is_ascii_alphabetic());
    if ok { Some(v.to_string()) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
