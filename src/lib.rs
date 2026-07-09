use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

const LANGUAGE_SERVER_ID: &str = "typescript";
const TYPESCRIPT_PACKAGE: &str = "typescript";
const TYPESCRIPT_BIN: &str = "node_modules/typescript/bin/tsc";
const UPDATE_CHANNEL_SETTING: &str = "updateChannel";

#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateChannel {
    Latest,
    Next,
}

struct TypeScriptExtension {
    installed_channel: Option<UpdateChannel>,
}

impl TypeScriptExtension {
    fn install_typescript(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String> {
        let update_channel = update_channel(worktree)?;

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let installed = zed::npm_package_installed_version(TYPESCRIPT_PACKAGE)?;
        let version = match update_channel {
            UpdateChannel::Latest => zed::npm_package_latest_version(TYPESCRIPT_PACKAGE)?,
            UpdateChannel::Next => "next".into(),
        };

        if update_channel == UpdateChannel::Next
            && self.installed_channel == Some(UpdateChannel::Next)
        {
            if let Some(installed) = installed.as_deref() {
                ensure_typescript_7_or_newer(installed)?;
            }
        } else if installed.as_deref() != Some(version.as_str()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::npm_install_package(TYPESCRIPT_PACKAGE, &version)?;
        }

        let installed = zed::npm_package_installed_version(TYPESCRIPT_PACKAGE)?;
        let installed = installed.as_deref().ok_or_else(|| {
            "TypeScript was not installed after npm install completed".to_string()
        })?;
        ensure_typescript_7_or_newer(installed)?;
        self.installed_channel = Some(update_channel);

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::None,
        );

        Ok(TYPESCRIPT_BIN.into())
    }
}

impl zed::Extension for TypeScriptExtension {
    fn new() -> Self {
        Self {
            installed_channel: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let package_bin = self.install_typescript(language_server_id, worktree)?;
        Ok(zed::Command {
            command: zed::node_binary_path()?,
            args: [package_bin].into_iter().chain(lsp_args()).collect(),
            env: worktree.shell_env(),
        })
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

fn lsp_args() -> Vec<String> {
    vec!["--lsp".into(), "--stdio".into()]
}

fn update_channel(worktree: &zed::Worktree) -> Result<UpdateChannel> {
    let Some(settings) = LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)
        .ok()
        .and_then(|settings| settings.settings)
    else {
        return Ok(UpdateChannel::Latest);
    };

    let Some(channel) = settings
        .as_object()
        .and_then(|settings| settings.get(UPDATE_CHANNEL_SETTING))
        .and_then(|channel| channel.as_str())
    else {
        return Ok(UpdateChannel::Latest);
    };

    match channel {
        "latest" => Ok(UpdateChannel::Latest),
        "next" => Ok(UpdateChannel::Next),
        _ => Err(format!(
            "unsupported TypeScript update channel `{channel}`; expected `latest` or `next`"
        )),
    }
}

fn strip_extension_settings(
    settings: Option<zed::serde_json::Value>,
) -> Option<zed::serde_json::Value> {
    let Some(settings) = settings else {
        return None;
    };

    match settings {
        zed::serde_json::Value::Object(mut object) => {
            object.remove(UPDATE_CHANNEL_SETTING);
            if object.is_empty() {
                None
            } else {
                Some(zed::serde_json::Value::Object(object))
            }
        }
        value => Some(value),
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

zed::register_extension!(TypeScriptExtension);
