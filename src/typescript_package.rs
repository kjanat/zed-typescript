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

/// Finds a usable project-local TypeScript 7+ package by scanning `package.json`
/// dependency sections. npm aliases may use any dependency key, but their
/// target package must be `typescript`.
pub fn find_local_typescript_package_dir(worktree: &zed::Worktree) -> Option<String> {
    let content = worktree.read_text_file("package.json").ok()?;
    let pkg: zed::serde_json::Value = zed::serde_json::from_str(&content).ok()?;

    let root = worktree
        .root_path()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();

    find_typescript_dependency(&pkg, &root)
}

fn find_typescript_dependency(pkg: &zed::serde_json::Value, root: &str) -> Option<String> {
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        let Some(dependencies) = pkg.get(section).and_then(|value| value.as_object()) else {
            continue;
        };

        for (key, value) in dependencies {
            let Some(spec) = value.as_str() else {
                continue;
            };
            if dependency_package_name(key, spec) != Some(TYPESCRIPT_PACKAGE) {
                continue;
            }

            let dir = format!("{root}/node_modules/{key}");
            let Ok(version) = typescript_version_from_package_dir(&dir) else {
                continue;
            };
            if ensure_typescript_7_or_newer(&version).is_ok()
                && has_usable_typescript_launcher(&dir)
            {
                return Some(dir);
            }
        }
    }

    None
}

fn dependency_package_name<'a>(key: &'a str, spec: &'a str) -> Option<&'a str> {
    let spec = spec.trim();
    let Some(alias) = spec.strip_prefix("npm:") else {
        return Some(key);
    };

    let version_separator = if let Some(scoped_alias) = alias.strip_prefix('@') {
        scoped_alias.rfind('@').map(|position| position + 1)
    } else {
        alias.rfind('@')
    };
    let package_name = version_separator.map_or(alias, |position| &alias[..position]);
    (!package_name.is_empty()).then_some(package_name)
}

fn has_usable_typescript_launcher(package_dir: &str) -> bool {
    node_shim_path(package_dir).is_ok() || find_native_server_binary(package_dir).is_some()
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
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct TestProject {
        root: PathBuf,
    }

    impl TestProject {
        fn new(name: &str) -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);

            let root = std::env::temp_dir().join(format!(
                "typescript-zed-{name}-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).expect("create test project directory");
            Self { root }
        }

        fn add_package(&self, key: &str, version: &str, has_launcher: bool) -> String {
            let package_dir = self.root.join("node_modules").join(key);
            std::fs::create_dir_all(&package_dir).expect("create test package directory");
            std::fs::write(
                package_dir.join("package.json"),
                format!(r#"{{"version":"{version}"}}"#),
            )
            .expect("write test package.json");

            if has_launcher {
                let bin_dir = package_dir.join("bin");
                std::fs::create_dir_all(&bin_dir).expect("create test package bin directory");
                std::fs::write(bin_dir.join("tsc"), "").expect("write test tsc launcher");
            }

            package_dir
                .to_string_lossy()
                .into_owned()
                .replace('\\', "/")
        }

        fn root(&self) -> String {
            self.root.to_string_lossy().into_owned().replace('\\', "/")
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn test_dependency_package_name() {
        let cases = [
            (("typescript", "^7.0.2"), Some("typescript")),
            (("typescript", "^8.0.0"), Some("typescript")),
            (
                ("@typescript/native", "npm:typescript@^7.0.2"),
                Some("typescript"),
            ),
            (("whatever", " npm:typescript@next "), Some("typescript")),
            (("foo", "^7.2.0"), Some("foo")),
            (("foo", "npm:bar@7.0.0"), Some("bar")),
            (
                ("typescript", "npm:@typescript/typescript6@^6.0.2"),
                Some("@typescript/typescript6"),
            ),
            (("foo", "npm:@scope/package@next"), Some("@scope/package")),
            (("foo", "npm:@scope/package"), Some("@scope/package")),
            (("foo", "npm:"), None),
        ];

        for ((key, spec), expected) in cases {
            assert_eq!(dependency_package_name(key, spec), expected);
        }
    }

    #[test]
    fn test_find_dependency_ignores_unrelated_7_and_accepts_typescript_8() {
        let project = TestProject::new("package-identity");
        project.add_package("foo", "7.2.0", true);
        let typescript_dir = project.add_package("typescript", "8.0.0", true);
        let manifest = zed::serde_json::json!({
            "dependencies": {
                "foo": "^7.2.0",
                "typescript": "^8.0.0"
            }
        });

        assert_eq!(
            find_typescript_dependency(&manifest, &project.root()),
            Some(typescript_dir)
        );
    }

    #[test]
    fn test_find_dependency_supports_side_by_side_aliases() {
        let project = TestProject::new("side-by-side-aliases");
        let native_dir = project.add_package("@typescript/native", "7.0.2", true);
        project.add_package("typescript", "6.0.2", true);
        let manifest = zed::serde_json::json!({
            "devDependencies": {
                "@typescript/native": "npm:typescript@^7.0.2",
                "typescript": "npm:@typescript/typescript6@^6.0.2"
            }
        });

        assert_eq!(
            find_typescript_dependency(&manifest, &project.root()),
            Some(native_dir)
        );
    }

    #[test]
    fn test_find_dependency_continues_after_outdated_alias() {
        let project = TestProject::new("continue-after-outdated");
        project.add_package("a-typescript", "6.0.2", true);
        let usable_dir = project.add_package("z-typescript", "7.0.2", true);
        let manifest = zed::serde_json::json!({
            "dependencies": {
                "a-typescript": "npm:typescript@6.0.2",
                "z-typescript": "npm:typescript@7.0.2"
            }
        });

        assert_eq!(
            find_typescript_dependency(&manifest, &project.root()),
            Some(usable_dir)
        );
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
    }
}
