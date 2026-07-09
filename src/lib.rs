mod settings;
mod typescript_package;

use settings::{ExtensionSetting, LANGUAGE_SERVER_ID};
use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

struct TypeScriptExtension {
    installed_spec: Option<String>,
}

impl TypeScriptExtension {
    /// Resolves the directory of the TypeScript 7+ package to run, preferring
    /// an explicit `tsdk.path`, then a project-local dependency, then a
    /// managed install into the extension's working directory.
    fn resolve_package_dir(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
        ext_settings: &Option<zed::serde_json::Value>,
    ) -> Result<String> {
        if let Some(tsdk_path) = settings::string_setting(ext_settings, ExtensionSetting::TsdkPath)?
        {
            let dir = typescript_package::tsdk_package_dir(worktree, &tsdk_path);
            let version = typescript_package::typescript_version_from_package_dir(&dir)
                .map_err(|error| format!("tsdk.path `{tsdk_path}` resolved to `{dir}`: {error}"))?;
            typescript_package::ensure_typescript_7_or_newer(&version)?;
            return Ok(dir);
        }

        if let Some(dir) = typescript_package::find_local_typescript_package_dir(worktree)
            && let Ok(version) = typescript_package::typescript_version_from_package_dir(&dir)
            && typescript_package::ensure_typescript_7_or_newer(&version).is_ok()
        {
            return Ok(dir);
        }
        // local dep was not 7+ (or not found), fall back to managed install for 7+

        self.install_managed(language_server_id, ext_settings)
    }

    fn install_managed(
        &mut self,
        language_server_id: &LanguageServerId,
        ext_settings: &Option<zed::serde_json::Value>,
    ) -> Result<String> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let requested = typescript_package::requested_typescript_spec(ext_settings)?;

        // fast path: if spec matches exactly (pinned version), skip queries and npm
        if self.installed_spec.as_deref() == Some(requested.install_spec.as_str()) {
            return typescript_package::managed_package_dir();
        }

        let package_dir =
            typescript_package::install_managed_typescript(language_server_id, &requested)?;
        self.installed_spec = Some(requested.install_spec);
        Ok(package_dir)
    }

    fn build_language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let ext_settings = settings::lsp_settings(worktree);
        let binary = LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)
            .ok()
            .and_then(|s| s.binary);
        let binary_env = binary.as_ref().and_then(|b| b.env.clone());

        // 1. user binary override (custom node e.g. volta shim, or direct tsc launcher)
        if let Some(path) = binary.as_ref().and_then(|b| b.path.clone()) {
            let user_args = binary.and_then(|b| b.arguments).unwrap_or_default();
            let (command, args) = if !user_args.is_empty() {
                (path, user_args)
            } else {
                let norm = path.replace('\\', "/");
                let is_tsc_launcher =
                    norm.ends_with("tsc") || norm.ends_with("tsc.js") || norm.ends_with("tsc.exe");
                if is_tsc_launcher {
                    (path, lsp_args(&ext_settings)?)
                } else {
                    // treat path as custom node; still resolve the tsc launcher + flags
                    // (never use which("tsc") — PATH tsc is often a volta/fnm/etc shim, not a raw JS to feed to node)
                    let package_dir =
                        self.resolve_package_dir(language_server_id, worktree, &ext_settings)?;
                    let shim = typescript_package::node_shim_path(&package_dir)?;
                    let args: Vec<String> = std::iter::once(shim)
                        .chain(lsp_args(&ext_settings)?)
                        .collect();
                    (path, args)
                }
            };
            let env = server_env(worktree, &ext_settings, binary_env)?;
            return Ok(zed::Command { command, args, env });
        }

        let package_dir = self.resolve_package_dir(language_server_id, worktree, &ext_settings)?;
        let args = lsp_args(&ext_settings)?;
        let env = server_env(worktree, &ext_settings, binary_env)?;

        // 2. run the native server binary directly when the platform package is
        //    resolvable — no Node process involved.
        if let Some(native) = typescript_package::find_native_server_binary(&package_dir) {
            return Ok(zed::Command {
                command: native,
                args,
                env,
            });
        }

        // 3. fall back to the package's Node launcher, which resolves the native
        //    binary via Node module resolution (covers pnpm and exotic layouts).
        //    Prefer the user's node (volta etc) via which, else Zed's bundled node.
        let node_cmd = if let Some(p) = worktree.which("node") {
            p
        } else {
            zed::node_binary_path()?
        };
        let shim = typescript_package::node_shim_path(&package_dir)?;
        let args: Vec<String> = std::iter::once(shim).chain(args).collect();

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
        Ok(settings::workspace_configuration(settings::lsp_settings(
            worktree,
        )))
    }

    fn label_for_completion(
        &self,
        _language_server_id: &LanguageServerId,
        completion: zed::lsp::Completion,
    ) -> Option<zed::CodeLabel> {
        use zed::lsp::CompletionKind as Kind;

        // mirror Zed's built-in TypeScript adapter: highlight the label by
        // completion kind, append the server's signature/description dimmed
        let highlight_name = match completion.kind? {
            Kind::Class | Kind::Interface | Kind::Enum | Kind::Constructor => "type",
            Kind::Constant => "constant",
            Kind::Function | Kind::Method => "function",
            Kind::Property | Kind::Field => "property",
            Kind::Variable => "variable",
            _ => return None,
        };

        let label = completion.label;
        let name_len = label.len();
        let mut code = label.clone();
        let mut spans = vec![zed::CodeLabelSpan::literal(
            label,
            Some(highlight_name.to_string()),
        )];

        // signature-style details render right after the name (e.g. `greet(name: string)`)
        if let Some(detail) = completion
            .label_details
            .as_ref()
            .and_then(|details| details.detail.as_ref())
        {
            code.push_str(detail);
            spans.push(zed::CodeLabelSpan::literal(detail.clone(), None));
        }

        // description (e.g. auto-import source) or item detail renders space-separated
        if let Some(description) = completion
            .label_details
            .as_ref()
            .and_then(|details| details.description.as_ref())
            .or(completion.detail.as_ref())
        {
            let suffix = format!(" {description}");
            code.push_str(&suffix);
            spans.push(zed::CodeLabelSpan::literal(suffix, None));
        }

        Some(zed::CodeLabel {
            code,
            spans,
            filter_range: (0..name_len).into(),
        })
    }
}

fn lsp_args(ext_settings: &Option<zed::serde_json::Value>) -> Result<Vec<String>> {
    let mut args = vec!["--lsp".into(), "--stdio".into()];

    if let Some(pprof_dir) = settings::string_setting(ext_settings, ExtensionSetting::PprofDir)? {
        args.push("--pprofDir".into());
        args.push(pprof_dir);
    }

    // escape hatch: forward any future server flags without an extension update
    if let Some(extra) = settings::string_array_setting(ext_settings, ExtensionSetting::ServerArgs)?
    {
        args.extend(extra);
    }

    Ok(args)
}

/// Builds the server environment. Precedence, lowest to highest: the user's
/// shell environment, `server.env`, `server.goMemLimit`, `binary.env`.
fn server_env(
    worktree: &zed::Worktree,
    ext_settings: &Option<zed::serde_json::Value>,
    binary_env: Option<std::collections::HashMap<String, String>>,
) -> Result<Vec<(String, String)>> {
    let mut env = worktree.shell_env();

    if let Some(extra) = settings::string_map_setting(ext_settings, ExtensionSetting::ServerEnv)? {
        for (key, value) in extra {
            upsert_env(&mut env, key, value);
        }
    }

    if let Some(go_mem_limit) =
        settings::string_setting(ext_settings, ExtensionSetting::GoMemLimit)?
    {
        settings::ensure_go_mem_limit(&go_mem_limit)?;
        upsert_env(&mut env, "GOMEMLIMIT".into(), go_mem_limit);
    }

    if let Some(binary_env) = binary_env {
        for (key, value) in binary_env {
            upsert_env(&mut env, key, value);
        }
    }

    Ok(env)
}

fn upsert_env(env: &mut Vec<(String, String)>, key: String, value: String) {
    env.retain(|(k, _)| *k != key);
    env.push((key, value));
}

zed::register_extension!(TypeScriptExtension);
