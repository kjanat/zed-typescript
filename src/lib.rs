use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

const LANGUAGE_SERVER_ID: &str = "typescript";
const TYPESCRIPT_PACKAGE: &str = "typescript";
const TYPESCRIPT_BIN: &str = "node_modules/typescript/bin/tsc";

struct TypeScriptExtension;

impl TypeScriptExtension {
    fn install_typescript(language_server_id: &LanguageServerId) -> Result<String> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let installed = zed::npm_package_installed_version(TYPESCRIPT_PACKAGE)?;
        let latest = zed::npm_package_latest_version(TYPESCRIPT_PACKAGE)?;
        ensure_typescript_7_or_newer(&latest)?;

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
