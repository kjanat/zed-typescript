use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

const LANGUAGE_SERVER_ID: &str = "typescript";
const TYPESCRIPT_PACKAGE: &str = "typescript";
const TYPESCRIPT_BIN: &str = "node_modules/typescript/bin/tsc";

struct TypeScriptExtension;

impl TypeScriptExtension {
    fn command_from_settings(worktree: &zed::Worktree) -> Option<zed::Command> {
        let settings = LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree).ok()?;
        let binary = settings.binary?;
        let path = binary.path?;
        let mut args = binary.arguments.unwrap_or_default();
        if args.is_empty() {
            args = lsp_args();
        }

        Some(zed::Command {
            command: path,
            args,
            env: worktree.shell_env(),
        })
    }

    fn command_from_preview_path(worktree: &zed::Worktree) -> Option<zed::Command> {
        worktree.which("tsgo").map(|command| zed::Command {
            command,
            args: lsp_args(),
            env: worktree.shell_env(),
        })
    }

    fn install_typescript(language_server_id: &LanguageServerId) -> Result<String> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let installed = zed::npm_package_installed_version(TYPESCRIPT_PACKAGE)?;
        let latest = zed::npm_package_latest_version(TYPESCRIPT_PACKAGE)?;

        if installed.as_deref() != Some(latest.as_str()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::npm_install_package(TYPESCRIPT_PACKAGE, &latest)?;
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::None,
        );

        Ok(TYPESCRIPT_BIN.into())
    }
}

impl zed::Extension for TypeScriptExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        if let Some(command) = Self::command_from_settings(worktree) {
            return Ok(command);
        }

        if let Some(command) = Self::command_from_preview_path(worktree) {
            return Ok(command);
        }

        let package_bin = Self::install_typescript(language_server_id)?;
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
        Ok(LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)
            .ok()
            .and_then(|settings| settings.settings))
    }
}

fn lsp_args() -> Vec<String> {
    vec!["--lsp".into(), "--stdio".into()]
}

zed::register_extension!(TypeScriptExtension);
