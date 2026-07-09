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

/// Normalizes a `tsdk.path` setting (VS Code convention: the package's `lib`
/// directory; also accepted: the package root or a `bin/tsc` path) into the
/// package root directory, resolved against the worktree when relative.
pub fn tsdk_package_dir(worktree: &zed::Worktree, tsdk_path: &str) -> String {
    let trimmed = tsdk_path.trim().trim_end_matches(['/', '\\']);
    let root = worktree
        .root_path()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    let base = if trimmed.starts_with('/') || trimmed.starts_with('\\') || trimmed.contains(':') {
        trimmed.to_string()
    } else {
        format!("{root}/{trimmed}")
    };

    let norm = base.replace('\\', "/");
    for suffix in ["/bin/tsc.js", "/bin/tsc", "/lib", "/bin"] {
        if let Some(stripped) = norm.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }
    norm
}

/// Finds a project-local TypeScript 7+ package by scanning `package.json`
/// dependency sections for anything that looks like a TypeScript 7 dependency
/// (including npm aliases such as `"typescript-7"` or `npm:typescript@7`).
pub fn find_local_typescript_package_dir(worktree: &zed::Worktree) -> Option<String> {
    let content = worktree.read_text_file("package.json").ok()?;
    let pkg: zed::serde_json::Value = zed::serde_json::from_str(&content).ok()?;

    let specifier_may_be_7 = |spec: &str| -> bool {
        let mut s = spec.trim().to_lowercase();
        // strip npm alias prefix like "npm:..." to get to the version part
        if let Some(pos) = s.find("npm:") {
            s = s[pos + 4..].to_string();
        }
        if s.contains("typescript6") || s.contains("/typescript6") {
            return false; // explicit 6 compat
        }
        // take part after last @ for the version specifier
        if let Some(at) = s.rfind('@') {
            s = s[at + 1..].to_string();
        }
        s == "*"
            || s == "latest"
            || s == "next"
            || s.starts_with("7")
            || s.starts_with("^7")
            || s.starts_with("~7")
            || s.starts_with(">=7")
            || s.contains("7.")
    };

    let mut possible_names = vec![];
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(o) = pkg.get(section).and_then(|d| d.as_object()) {
            for (key, val) in o {
                if let Some(v) = val.as_str()
                    && specifier_may_be_7(v)
                {
                    possible_names.push(key.clone());
                }
            }
        }
    }

    let root = worktree
        .root_path()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    for name in possible_names {
        let dir = format!("{root}/node_modules/{name}");
        if std::fs::metadata(format!("{dir}/package.json")).is_ok_and(|m| m.is_file()) {
            return Some(dir);
        }
    }

    None
}

pub fn typescript_version_from_package_dir(package_dir: &str) -> Result<String> {
    let pkg_json = format!("{package_dir}/package.json");
    let content = std::fs::read_to_string(&pkg_json)
        .map_err(|error| format!("failed to read {pkg_json}: {error}"))?;
    let pkg: zed::serde_json::Value = zed::serde_json::from_str(&content)
        .map_err(|error| format!("invalid {pkg_json}: {error}"))?;
    pkg.get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("no version in {pkg_json}"))
}

/// Locates the native `tsc` executable that ships in the per-platform
/// `@typescript/typescript-<platform>-<arch>` package next to (or inside) the
/// resolved `typescript` package. Returns `None` when the platform has no
/// prebuilt binary or the package layout is not recognized (pnpm virtual
/// stores, unusual hoisting) — callers fall back to running the package's
/// `bin/tsc` Node shim, which performs Node module resolution instead.
pub fn find_native_server_binary(package_dir: &str) -> Option<String> {
    let (os, arch) = zed::current_platform();
    let platform = match os {
        zed::Os::Mac => "darwin",
        zed::Os::Linux => "linux",
        zed::Os::Windows => "win32",
    };
    let arch = match arch {
        zed::Architecture::Aarch64 => "arm64",
        zed::Architecture::X8664 => "x64",
        zed::Architecture::X86 => return None,
    };
    let exe = match os {
        zed::Os::Windows => "tsc.exe",
        _ => "tsc",
    };

    let platform_package = format!("@typescript/typescript-{platform}-{arch}");
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
