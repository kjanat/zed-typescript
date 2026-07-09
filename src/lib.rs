use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

const LANGUAGE_SERVER_ID: &str = "typescript";
const TYPESCRIPT_PACKAGE: &str = "typescript";
const TYPESCRIPT_BIN: &str = "node_modules/typescript/bin/tsc";
const VERSION_SETTING: &str = "version";
const UPDATE_CHANNEL_SETTING: &str = "updateChannel";
const TSDK_PATH_SETTING: &str = "tsdk.path";
const PPROF_DIR_SETTING: &str = "server.pprofDir";
const GO_MEM_LIMIT_SETTING: &str = "server.goMemLimit";

#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateChannel {
    Latest,
    Next,
}

struct TypeScriptExtension {
    installed_channel: Option<UpdateChannel>,
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

        let installed = zed::npm_package_installed_version(TYPESCRIPT_PACKAGE)?;
        let install_spec = requested.install_spec.clone();

        if self.installed_spec.as_deref() == Some(install_spec.as_str())
            || requested.matches_installed(installed.as_deref())
        {
            if let Some(installed) = installed.as_deref() {
                ensure_typescript_7_or_newer(installed)?;
            }
        } else {
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
        self.installed_channel = requested.update_channel;
        self.installed_spec = Some(install_spec);

        Ok(TYPESCRIPT_BIN.into())
    }

    fn build_language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let settings = lsp_settings(worktree);
        let package_bin = if let Some(tsdk_path) = string_setting(&settings, TSDK_PATH_SETTING) {
            tsdk_bin_path(worktree, tsdk_path)
        } else {
            self.install_typescript(language_server_id, worktree)?
        };

        Ok(zed::Command {
            command: zed::node_binary_path()?,
            args: [package_bin]
                .into_iter()
                .chain(lsp_args(&settings))
                .collect(),
            env: server_env(worktree, &settings)?,
        })
    }
}

impl zed::Extension for TypeScriptExtension {
    fn new() -> Self {
        Self {
            installed_channel: None,
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

    if let Some(pprof_dir) = string_setting(settings, PPROF_DIR_SETTING) {
        args.push("--pprofDir".into());
        args.push(pprof_dir.into());
    }

    args
}

struct RequestedTypescriptSpec {
    install_spec: String,
    exact_version: Option<String>,
    update_channel: Option<UpdateChannel>,
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

    if let Some(version) = string_setting(&settings, VERSION_SETTING) {
        ensure_non_empty_version(version)?;
        return Ok(RequestedTypescriptSpec {
            install_spec: version.into(),
            exact_version: exact_version(version),
            update_channel: None,
        });
    }

    let Some(channel) = string_setting(&settings, UPDATE_CHANNEL_SETTING) else {
        let latest = zed::npm_package_latest_version(TYPESCRIPT_PACKAGE)?;
        ensure_typescript_7_or_newer(&latest)?;
        return Ok(RequestedTypescriptSpec {
            install_spec: latest.clone(),
            exact_version: Some(latest),
            update_channel: Some(UpdateChannel::Latest),
        });
    };

    match channel {
        "latest" => {
            let latest = zed::npm_package_latest_version(TYPESCRIPT_PACKAGE)?;
            ensure_typescript_7_or_newer(&latest)?;
            Ok(RequestedTypescriptSpec {
                install_spec: latest.clone(),
                exact_version: Some(latest),
                update_channel: Some(UpdateChannel::Latest),
            })
        }
        "next" => Ok(RequestedTypescriptSpec {
            install_spec: "next".into(),
            exact_version: None,
            update_channel: Some(UpdateChannel::Next),
        }),
        _ => Err(format!(
            "unsupported TypeScript update channel `{channel}`; expected `latest` or `next`"
        )),
    }
}

fn tsdk_bin_path(worktree: &zed::Worktree, tsdk_path: &str) -> String {
    let path = if tsdk_path.starts_with('/') {
        tsdk_path.to_string()
    } else {
        format!("{}/{}", worktree.root_path(), tsdk_path)
    };

    if path.ends_with("/bin/tsc") || path.ends_with("/bin/tsc.js") {
        path
    } else if path.ends_with("/lib") {
        format!("{}/../bin/tsc", path)
    } else {
        format!("{}/bin/tsc", path)
    }
}

fn server_env(
    worktree: &zed::Worktree,
    settings: &Option<zed::serde_json::Value>,
) -> Result<Vec<(String, String)>> {
    let mut env = worktree.shell_env();

    if let Some(go_mem_limit) = string_setting(settings, GO_MEM_LIMIT_SETTING) {
        ensure_go_mem_limit(go_mem_limit)?;
        env.push(("GOMEMLIMIT".into(), go_mem_limit.into()));
    }

    Ok(env)
}

fn string_setting<'a>(
    settings: &'a Option<zed::serde_json::Value>,
    dotted_path: &str,
) -> Option<&'a str> {
    let settings = settings.as_ref()?.as_object()?;

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
            object.remove(VERSION_SETTING);
            object.remove(UPDATE_CHANNEL_SETTING);
            remove_setting(&mut object, TSDK_PATH_SETTING);
            remove_setting(&mut object, PPROF_DIR_SETTING);
            remove_setting(&mut object, GO_MEM_LIMIT_SETTING);
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
    if version
        .chars()
        .all(|character| character.is_ascii_digit() || character == '.')
    {
        Some(version.into())
    } else if version.starts_with('v')
        && version[1..]
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
    {
        Some(version[1..].into())
    } else {
        None
    }
}

fn remove_setting(object: &mut zed::serde_json::Map<String, zed::serde_json::Value>, path: &str) {
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
    let major = version
        .strip_prefix('v')
        .unwrap_or(version)
        .split('.')
        .next()
        .ok_or_else(|| format!("invalid TypeScript version `{version}`"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid TypeScript version `{version}`"))?;

    if major < 7 {
        return Err(format!(
            "TypeScript LSP requires TypeScript 7 or newer, got `{version}`"
        ));
    }

    Ok(())
}

fn ensure_go_mem_limit(value: &str) -> Result<()> {
    let Some(first_suffix_char) = value.find(|character: char| !character.is_ascii_digit()) else {
        return Ok(());
    };

    if first_suffix_char == 0 {
        return Err(format!("invalid GOMEMLIMIT value `{value}`"));
    }

    match &value[first_suffix_char..] {
        "B" | "KB" | "MB" | "GB" | "TB" | "KiB" | "MiB" | "GiB" | "TiB" => Ok(()),
        _ => Err(format!("invalid GOMEMLIMIT value `{value}`")),
    }
}

zed::register_extension!(TypeScriptExtension);
