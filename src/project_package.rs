use crate::host_platform::Platform;
use zed_extension_api::{self as zed, Result};

const RESOLVER: &str = include_str!("resolve_typescript.cjs");
const RESOLVER_BOOTSTRAP: &str = include_str!("run_resolver.cjs");

/// Checked host paths. Never re-check project paths with sandboxed `std::fs`.
pub struct ResolvedPackage {
    pub directory: String,
    pub native: Option<String>,
    pub shim: Option<String>,
}

pub struct Discovery {
    pub platform: Platform,
    pub package: Option<ResolvedPackage>,
}

pub fn node_binary(worktree: &zed::Worktree) -> Result<String> {
    // In particular, FreeBSD must use a host-installed Node: Zed's managed
    // Node downloader does not support that OS. Do not call current_platform.
    match worktree.which("node") {
        Some(node) => Ok(node),
        None => zed::node_binary_path().map_err(|_| {
            "No usable Node runtime for host discovery. Install Node on the server host and make it available on the worktree PATH; FreeBSD requires a host-installed Node.".into()
        }),
    }
}

impl ResolvedPackage {
    pub fn node_shim(&self) -> Result<String> {
        self.shim.clone().ok_or_else(|| {
            format!(
                "TypeScript package at `{}` has no `bin/tsc` launcher",
                self.directory
            )
        })
    }
}

pub fn resolve(worktree: &zed::Worktree, tsdk: Option<&str>) -> Result<Discovery> {
    if tsdk.is_some_and(|path| path.trim().is_empty()) {
        return Err("tsdk.path must not be empty".into());
    }
    // Even without a project package, the managed installer needs the host's
    // platform. Obtain both results in one short-lived host process.
    let output = zed::process::Command::new(node_binary(worktree)?)
        .args([
            "--input-type=commonjs",
            "--eval",
            RESOLVER_BOOTSTRAP.trim(),
            "--",
            "--zed-typescript-resolve",
            &worktree.root_path(),
            tsdk.unwrap_or_default(),
        ])
        .envs(worktree.shell_env())
        // Volta forwards Node through cmd.exe on Windows, which truncates
        // multiline --eval arguments. Keep source out of the command line.
        .env("ZED_TYPESCRIPT_RESOLVER", RESOLVER)
        .output()
        .map_err(|_| "Could not run the TypeScript package resolver; check the extension's process:exec permission".to_string())?;
    if output.status != Some(0) {
        return Err(resolver_error(&output.stdout, tsdk));
    }
    decode(&output.stdout)
}

fn resolver_error(stdout: &[u8], tsdk: Option<&str>) -> String {
    if let Ok(value) = zed::serde_json::from_slice::<zed::serde_json::Value>(stdout)
        && let Some(message) = value.get("error").and_then(|value| value.as_str())
        && !message.trim().is_empty()
    {
        return message.to_string();
    }
    match tsdk {
        Some(path) => {
            format!("tsdk.path `{path}` could not be resolved to a usable TypeScript 7+ package")
        }
        None => "TypeScript project package resolution failed".into(),
    }
}

fn decode(stdout: &[u8]) -> Result<Discovery> {
    let value: zed::serde_json::Value = zed::serde_json::from_slice(stdout)
        .map_err(|_| "Invalid response from the TypeScript package resolver".to_string())?;
    let platform = value
        .get("platform")
        .ok_or_else(|| "Missing platform in TypeScript resolver response".to_string())?;
    let os = platform
        .get("os")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let arch = platform
        .get("arch")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let platform = Platform::new(os, arch)?;
    let package = value
        .get("package")
        .ok_or_else(|| "Missing package result in TypeScript resolver response".to_string())?;
    Ok(Discovery {
        platform,
        package: decode_package(package)?,
    })
}

fn decode_package(value: &zed::serde_json::Value) -> Result<Option<ResolvedPackage>> {
    if value.is_null() {
        return Ok(None);
    }
    let field = |key| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let directory = field("packageDirectory")
        .ok_or_else(|| "Missing package directory in TypeScript resolver response".to_string())?;
    let package = ResolvedPackage {
        directory,
        native: field("native"),
        shim: field("shim"),
    };
    if package.native.is_none() && package.shim.is_none() {
        return Err("Missing launcher in TypeScript resolver response".into());
    }
    Ok(Some(package))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_errors_preserve_details_and_handle_missing_output() {
        let message = "tsdk.path must point to an installed TypeScript 7+ package with a launcher for this platform";
        let stdout = zed::serde_json::to_vec(&zed::serde_json::json!({"error": message})).unwrap();
        assert_eq!(resolver_error(&stdout, Some("missing")), message);
        for invalid in [
            "",
            "not json",
            "null",
            "{}",
            r#"{"error":null}"#,
            r#"{"error":42}"#,
            r#"{"error":"  "}"#,
        ] {
            assert_eq!(
                resolver_error(invalid.as_bytes(), Some("missing")),
                "tsdk.path `missing` could not be resolved to a usable TypeScript 7+ package"
            );
            assert_eq!(
                resolver_error(invalid.as_bytes(), None),
                "TypeScript project package resolution failed"
            );
        }
    }

    #[test]
    fn decode_distinguishes_missing_packages_from_invalid_responses() {
        assert!(
            decode(br#"{"platform":{"os":"freebsd","arch":"x64"},"package":null}"#)
                .unwrap()
                .package
                .is_none()
        );
        assert!(decode(b"null").is_err());
        assert!(decode(b"{}").is_err());
        assert!(decode(br#"{"packageDirectory":"/project/typescript"}"#).is_err());
        assert!(decode(b"not json").is_err());
    }

    #[test]
    fn native_only_packages_cannot_be_passed_to_custom_node() {
        let discovery = decode(br#"{"platform":{"os":"freebsd","arch":"arm64"},"package":{"packageDirectory":"/project/typescript","native":"/project/native/tsc","shim":null}}"#)
            .unwrap();
        assert_eq!(discovery.platform.name, "freebsd-arm64");
        let package = discovery.package.unwrap();
        assert_eq!(package.native.as_deref(), Some("/project/native/tsc"));
        assert!(package.node_shim().is_err());
    }
}
