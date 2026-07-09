use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

const LANGUAGE_SERVER_ID: &str = "typescript";
const TYPESCRIPT_PACKAGE: &str = "typescript";
const TYPESCRIPT_BIN: &str = "node_modules/typescript/bin/tsc";

const EXTENSION_SETTINGS: [ExtensionSetting; 5] = [
    ExtensionSetting::Version,
    ExtensionSetting::UpdateChannel,
    ExtensionSetting::TsdkPath,
    ExtensionSetting::PprofDir,
    ExtensionSetting::GoMemLimit,
];

#[derive(Clone, Copy)]
enum ExtensionSetting {
    Version,
    UpdateChannel,
    TsdkPath,
    PprofDir,
    GoMemLimit,
}

impl ExtensionSetting {
    fn path(self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::UpdateChannel => "updateChannel",
            Self::TsdkPath => "tsdk.path",
            Self::PprofDir => "server.pprofDir",
            Self::GoMemLimit => "server.goMemLimit",
        }
    }
}

struct TypeScriptExtension {
    installed_spec: Option<String>,
}

impl TypeScriptExtension {
    fn install_typescript(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let requested = requested_typescript_spec(worktree)?;
        let install_spec = requested.install_spec.clone();

        // fast path: if spec matches exactly (pinned version), skip queries and npm
        if self.installed_spec.as_deref() == Some(install_spec.as_str()) {
            return extension_file_path(TYPESCRIPT_BIN);
        }

        let current = zed::npm_package_installed_version(TYPESCRIPT_PACKAGE)?;
        let is_tag = requested.exact_version.is_none();
        let needs_install = is_tag || !requested.matches_installed(current.as_deref());

        if needs_install {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::npm_install_package(TYPESCRIPT_PACKAGE, &install_spec)?;
        }

        let installed = zed::npm_package_installed_version(TYPESCRIPT_PACKAGE)?;
        let installed = installed.as_deref().ok_or_else(|| {
            "TypeScript was not installed after npm install completed".to_string()
        })?;
        ensure_typescript_7_or_newer(installed)?;
        self.installed_spec = Some(install_spec);

        extension_file_path(TYPESCRIPT_BIN)
    }

    fn resolve_typescript_bin(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
        ext_settings: &Option<zed::serde_json::Value>,
    ) -> Result<String> {
        if let Some(tsdk_path) = string_setting(ext_settings, ExtensionSetting::TsdkPath) {
            let p = tsdk_bin_path(worktree, tsdk_path);
            if !std::fs::metadata(&p).is_ok_and(|m| m.is_file()) {
                return Err(format!(
                    "tsdk.path `{}` resolved to `{}`, which does not exist",
                    tsdk_path, p
                ));
            }
            let ver = typescript_version_from_bin(&p)?;
            ensure_typescript_7_or_newer(&ver)?;
            return Ok(p);
        }

        if let Some(local) = find_local_typescript_bin(worktree)
            && let Ok(ver) = typescript_version_from_bin(&local)
            && ensure_typescript_7_or_newer(&ver).is_ok()
        {
            return Ok(local);
        }
        // local dep was not 7+ (or not found), fall back to managed install for 7+

        self.install_typescript(language_server_id, worktree)
    }

    fn build_language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let ext_settings = lsp_settings(worktree);
        let full_lsp = LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree).ok();

        // 1. user binary override (custom node e.g. volta shim, or direct tsc shim)
        if let Some(binary) = full_lsp.as_ref().and_then(|s| s.binary.as_ref())
            && let Some(path) = &binary.path
        {
            let user_args = binary.arguments.clone().unwrap_or_default();
            let (command, args) = if !user_args.is_empty() {
                (path.clone(), user_args)
            } else {
                let norm = path.replace('\\', "/");
                let is_tsc_script = norm.ends_with("/tsc")
                    || norm.ends_with("/tsc.js")
                    || norm.ends_with("tsc")
                    || norm.ends_with("tsc.js");
                if is_tsc_script {
                    (path.clone(), lsp_args(&ext_settings))
                } else {
                    // treat path as custom node; still resolve tsc script + flags
                    // (never use which("tsc") — PATH tsc is often a volta/fnm/etc shim, not a raw JS to feed to node)
                    let tsc =
                        self.resolve_typescript_bin(language_server_id, worktree, &ext_settings)?;
                    let a: Vec<String> = std::iter::once(tsc)
                        .chain(lsp_args(&ext_settings))
                        .collect();
                    (path.clone(), a)
                }
            };
            let env = server_env(worktree, &ext_settings)?;
            return Ok(zed::Command { command, args, env });
        }

        // 2. prefer user's node (volta etc) via which, else Zed's bundled node.
        //    The tsc script is always resolved via tsdk.path or the managed npm install
        //    (never PATH "tsc", which is usually a shell shim).
        let node_cmd = if let Some(p) = worktree.which("node") {
            p
        } else {
            zed::node_binary_path()?
        };

        let package_bin =
            self.resolve_typescript_bin(language_server_id, worktree, &ext_settings)?;

        let args: Vec<String> = std::iter::once(package_bin)
            .chain(lsp_args(&ext_settings))
            .collect();
        let env = server_env(worktree, &ext_settings)?;

        Ok(zed::Command {
            command: node_cmd,
            args,
            env,
        })
    }
}

impl zed::Extension for TypeScriptExtension {
    fn new() -> Self {
        Self {
            installed_spec: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        match self.build_language_server_command(language_server_id, worktree) {
            Ok(command) => {
                zed::set_language_server_installation_status(
                    language_server_id,
                    &zed::LanguageServerInstallationStatus::None,
                );
                Ok(command)
            }
            Err(error) => {
                zed::set_language_server_installation_status(
                    language_server_id,
                    &zed::LanguageServerInstallationStatus::Failed(error.clone()),
                );
                Err(error)
            }
        }
    }

    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)
            .ok()
            .and_then(|settings| settings.initialization_options))
    }

    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        let settings = LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)
            .ok()
            .and_then(|settings| settings.settings);

        Ok(strip_extension_settings(settings))
    }
}

fn lsp_settings(worktree: &zed::Worktree) -> Option<zed::serde_json::Value> {
    LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)
        .ok()
        .and_then(|settings| settings.settings)
}

fn lsp_args(settings: &Option<zed::serde_json::Value>) -> Vec<String> {
    let mut args = vec!["--lsp".into(), "--stdio".into()];

    if let Some(pprof_dir) = string_setting(settings, ExtensionSetting::PprofDir) {
        args.push("--pprofDir".into());
        args.push(pprof_dir.into());
    }

    args
}

struct RequestedTypescriptSpec {
    install_spec: String,
    exact_version: Option<String>,
}

impl RequestedTypescriptSpec {
    fn matches_installed(&self, installed: Option<&str>) -> bool {
        self.exact_version.as_deref().is_some_and(|exact_version| {
            installed.is_some_and(|installed| installed == exact_version)
        })
    }
}

fn requested_typescript_spec(worktree: &zed::Worktree) -> Result<RequestedTypescriptSpec> {
    let settings = lsp_settings(worktree);

    if let Some(version) = string_setting(&settings, ExtensionSetting::Version) {
        let version = version.trim();
        ensure_non_empty_version(version)?;
        return Ok(RequestedTypescriptSpec {
            install_spec: version.to_string(),
            exact_version: exact_version(version),
        });
    }

    let Some(channel) = string_setting(&settings, ExtensionSetting::UpdateChannel) else {
        let latest = zed::npm_package_latest_version(TYPESCRIPT_PACKAGE)?;
        ensure_typescript_7_or_newer(&latest)?;
        return Ok(RequestedTypescriptSpec {
            install_spec: latest.clone(),
            exact_version: Some(latest),
        });
    };

    match channel {
        "latest" => {
            let latest = zed::npm_package_latest_version(TYPESCRIPT_PACKAGE)?;
            ensure_typescript_7_or_newer(&latest)?;
            Ok(RequestedTypescriptSpec {
                install_spec: latest.clone(),
                exact_version: Some(latest),
            })
        }
        "next" => Ok(RequestedTypescriptSpec {
            install_spec: "next".to_string(),
            exact_version: None,
        }),
        _ => Err(format!(
            "unsupported TypeScript update channel `{channel}`; expected `latest` or `next`"
        )),
    }
}

fn tsdk_bin_path(worktree: &zed::Worktree, tsdk_path: &str) -> String {
    let trimmed = tsdk_path.trim().trim_end_matches(['/', '\\']).to_string();
    let root = worktree.root_path();
    let root_trim = root.trim_end_matches(['/', '\\']);
    let base = if trimmed.starts_with('/') || trimmed.starts_with('\\') || trimmed.contains(':') {
        trimmed.clone()
    } else {
        format!("{}/{}", root_trim, trimmed)
    };

    let norm = base.replace('\\', "/");
    if norm.ends_with("/bin/tsc") || norm.ends_with("/bin/tsc.js") {
        base.replace('\\', "/")
    } else if norm.ends_with("/lib") {
        let b = base.trim_end_matches(['/', '\\']);
        format!("{}/../bin/tsc", b).replace('\\', "/")
    } else {
        let b = base.trim_end_matches(['/', '\\']);
        format!("{}/bin/tsc", b).replace('\\', "/")
    }
}

fn extension_file_path(path: &str) -> Result<String> {
    let path = std::env::current_dir()
        .map_err(|error| format!("failed to read extension directory: {error}"))?
        .join(path);

    Ok(path.to_string_lossy().into_owned())
}

fn server_env(
    worktree: &zed::Worktree,
    settings: &Option<zed::serde_json::Value>,
) -> Result<Vec<(String, String)>> {
    let mut env = worktree.shell_env();

    if let Some(go_mem_limit) = string_setting(settings, ExtensionSetting::GoMemLimit) {
        ensure_go_mem_limit(go_mem_limit)?;
        env.retain(|(k, _)| k != "GOMEMLIMIT");
        env.push(("GOMEMLIMIT".into(), go_mem_limit.into()));
    }

    Ok(env)
}

fn string_setting(
    settings: &Option<zed::serde_json::Value>,
    setting: ExtensionSetting,
) -> Option<&str> {
    let settings = settings.as_ref()?.as_object()?;
    let dotted_path = setting.path();

    let mut parts = dotted_path.split('.');
    let first = parts.next()?;
    if let Some(mut value) = settings.get(first) {
        for part in parts {
            value = value.as_object()?.get(part)?;
        }

        if let Some(value) = value.as_str() {
            return Some(value);
        }
    }

    settings.get(dotted_path).and_then(|value| value.as_str())
}

fn strip_extension_settings(
    settings: Option<zed::serde_json::Value>,
) -> Option<zed::serde_json::Value> {
    let settings = settings?;

    match settings {
        zed::serde_json::Value::Object(mut object) => {
            for setting in EXTENSION_SETTINGS {
                remove_setting(&mut object, setting);
            }
            if object.is_empty() {
                None
            } else {
                Some(zed::serde_json::Value::Object(object))
            }
        }
        value => Some(value),
    }
}

fn ensure_non_empty_version(version: &str) -> Result<()> {
    if version.trim().is_empty() {
        return Err("TypeScript version setting must not be empty".into());
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

fn remove_setting(
    object: &mut zed::serde_json::Map<String, zed::serde_json::Value>,
    setting: ExtensionSetting,
) {
    let path = setting.path();
    object.remove(path);

    let mut parts = path.split('.');
    let Some(first) = parts.next() else {
        return;
    };
    let Some(parent) = object
        .get_mut(first)
        .and_then(|value| value.as_object_mut())
    else {
        return;
    };
    let Some(last) = parts.next() else {
        return;
    };

    parent.remove(last);
    if parent.is_empty() {
        object.remove(first);
    }
}

fn ensure_typescript_7_or_newer(version: &str) -> Result<()> {
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

fn ensure_go_mem_limit(value: &str) -> Result<()> {
    // allow digits and . for decimal like 1.5GiB
    let Some(first_suffix_char) =
        value.find(|character: char| !(character.is_ascii_digit() || character == '.'))
    else {
        return Ok(());
    };

    if first_suffix_char == 0 {
        return Err(format!("invalid GOMEMLIMIT value `{value}`"));
    }

    let num = &value[..first_suffix_char];
    if num.is_empty()
        || num.starts_with('.')
        || num.ends_with('.')
        || num.chars().filter(|&c| c == '.').count() > 1
    {
        return Err(format!("invalid GOMEMLIMIT value `{value}`"));
    }

    match &value[first_suffix_char..] {
        "B" | "KB" | "MB" | "GB" | "TB" | "KiB" | "MiB" | "GiB" | "TiB" => Ok(()),
        _ => Err(format!("invalid GOMEMLIMIT value `{value}`")),
    }
}

fn find_local_typescript_bin(worktree: &zed::Worktree) -> Option<String> {
    // check for any dep (regular/dev/peer) that looks like a typescript-7 (including aliases like "typescript-7", "@typescript/native")
    if let Ok(content) = worktree.read_text_file("package.json")
        && let Ok(pkg) = zed::serde_json::from_str::<zed::serde_json::Value>(&content)
    {
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
                || s == "7"
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

        if possible_names.is_empty() {
            return None;
        }

        let root = worktree.root_path();
        for name in possible_names {
            let bin = format!("{}/node_modules/{}/bin/tsc", root, name);
            if std::fs::metadata(&bin).is_ok_and(|m| m.is_file()) {
                return Some(bin);
            }
        }
    }

    None
}

fn typescript_version_from_bin(bin_path: &str) -> Result<String> {
    let pkg_dir = std::path::Path::new(bin_path)
        .parent()
        .and_then(|b| b.parent())
        .ok_or_else(|| "invalid tsc bin path".to_string())?;
    let pkg_json = pkg_dir.join("package.json");
    let content = std::fs::read_to_string(&pkg_json)
        .map_err(|e| format!("failed to read {}: {e}", pkg_json.display()))?;
    let pkg: zed::serde_json::Value = zed::serde_json::from_str(&content)
        .map_err(|e| format!("invalid {}: {e}", pkg_json.display()))?;
    pkg.get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("no version in {}", pkg_json.display()))
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

    #[test]
    fn test_go_mem_limit() {
        assert!(ensure_go_mem_limit("2048MiB").is_ok());
        assert!(ensure_go_mem_limit("1.5GiB").is_ok());
        assert!(ensure_go_mem_limit("1024").is_ok());
        assert!(ensure_go_mem_limit("1.2.3GiB").is_err());
        assert!(ensure_go_mem_limit("foo").is_err());
        assert!(ensure_go_mem_limit(".5GiB").is_err());
    }
}

zed::register_extension!(TypeScriptExtension);
