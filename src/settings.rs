use zed_extension_api::{self as zed, Result, settings::LspSettings};

pub const LANGUAGE_SERVER_ID: &str = "typescript";

/// Settings under `lsp.typescript.settings` owned by this extension rather than
/// the language server. Anything whose top-level key (or dotted-key prefix)
/// matches one of these is consumed here and stripped before the rest is
/// forwarded to the server as `workspace/configuration`.
const EXTENSION_KEY_PREFIXES: [&str; 4] = ["version", "updateChannel", "tsdk", "server"];

#[derive(Clone, Copy)]
pub enum ExtensionSetting {
    Version,
    UpdateChannel,
    TsdkPath,
    PprofDir,
    GoMemLimit,
    ServerArgs,
    ServerEnv,
}

impl ExtensionSetting {
    fn path(self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::UpdateChannel => "updateChannel",
            Self::TsdkPath => "tsdk.path",
            Self::PprofDir => "server.pprofDir",
            Self::GoMemLimit => "server.goMemLimit",
            Self::ServerArgs => "server.args",
            Self::ServerEnv => "server.env",
        }
    }
}

pub fn lsp_settings(worktree: &zed::Worktree) -> Option<zed::serde_json::Value> {
    LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)
        .ok()
        .and_then(|settings| settings.settings)
}

/// Looks a setting up by its dotted path, accepting both the nested form
/// (`{"server": {"pprofDir": ...}}`) and the literal dotted key
/// (`{"server.pprofDir": ...}`). The nested form wins when both are set.
fn setting_value(
    settings: &Option<zed::serde_json::Value>,
    setting: ExtensionSetting,
) -> Option<&zed::serde_json::Value> {
    let object = settings.as_ref()?.as_object()?;
    let dotted_path = setting.path();

    let mut parts = dotted_path.split('.');
    let first = parts.next()?;
    let nested = object.get(first).and_then(|mut value| {
        for part in parts {
            value = value.as_object()?.get(part)?;
        }
        Some(value)
    });

    nested.or_else(|| object.get(dotted_path))
}

pub fn string_setting(
    settings: &Option<zed::serde_json::Value>,
    setting: ExtensionSetting,
) -> Result<Option<String>> {
    match setting_value(settings, setting) {
        None => Ok(None),
        Some(value) => value.as_str().map(|s| Some(s.to_string())).ok_or_else(|| {
            format!(
                "setting `{}` must be a string, got: {value}",
                setting.path()
            )
        }),
    }
}

pub fn string_array_setting(
    settings: &Option<zed::serde_json::Value>,
    setting: ExtensionSetting,
) -> Result<Option<Vec<String>>> {
    let Some(value) = setting_value(settings, setting) else {
        return Ok(None);
    };
    let error = || {
        format!(
            "setting `{}` must be an array of strings, got: {value}",
            setting.path()
        )
    };
    let items = value.as_array().ok_or_else(error)?;
    items
        .iter()
        .map(|item| item.as_str().map(|s| s.to_string()).ok_or_else(error))
        .collect::<Result<Vec<String>>>()
        .map(Some)
}

pub fn string_map_setting(
    settings: &Option<zed::serde_json::Value>,
    setting: ExtensionSetting,
) -> Result<Option<Vec<(String, String)>>> {
    let Some(value) = setting_value(settings, setting) else {
        return Ok(None);
    };
    let object = value.as_object().ok_or_else(|| {
        format!(
            "setting `{}` must be an object of string values, got: {value}",
            setting.path()
        )
    })?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|s| (key.clone(), s.to_string()))
                .ok_or_else(|| {
                    format!(
                        "setting `{}.{key}` must be a string, got: {value}",
                        setting.path()
                    )
                })
        })
        .collect::<Result<Vec<(String, String)>>>()
        .map(Some)
}

/// Server-side preference defaults, mirroring Zed's built-in vtsls adapter so
/// inlay hints and code lenses work out of the box once enabled in Zed. Only
/// keys the TypeScript 7 server actually parses are included; user settings
/// deep-merge over these with the user winning at leaf level.
fn default_workspace_configuration() -> zed::serde_json::Value {
    let language_defaults = zed::serde_json::json!({
        "inlayHints": {
            "parameterNames": { "enabled": "all", "suppressWhenArgumentMatchesName": false },
            "parameterTypes": { "enabled": true },
            "variableTypes": { "enabled": true, "suppressWhenTypeMatchesName": false },
            "propertyDeclarationTypes": { "enabled": true },
            "functionLikeReturnTypes": { "enabled": true },
            "enumMemberValues": { "enabled": true },
        },
        "implementationsCodeLens": { "enabled": true, "showOnAllClassMethods": true, "showOnInterfaceMethods": true },
        "referencesCodeLens": { "enabled": true, "showOnAllFunctions": true },
    });

    zed::serde_json::json!({
        "typescript": language_defaults,
        "javascript": language_defaults,
    })
}

/// Builds the JSON served to the language server's `workspace/configuration`
/// requests: extension-owned settings are stripped, remaining VS Code-style
/// dotted keys (`"typescript.inlayHints.parameterNames.enabled"`) are expanded
/// into the nested sections the server reads (`js/ts`, `typescript`,
/// `javascript`, `editor`), and the result is deep-merged over the defaults
/// (user wins). Nested keys win over dotted keys on conflict.
pub fn workspace_configuration(
    settings: Option<zed::serde_json::Value>,
) -> Option<zed::serde_json::Value> {
    let mut configuration = default_workspace_configuration();

    let Some(settings) = settings else {
        return Some(configuration);
    };

    let zed::serde_json::Value::Object(object) = settings else {
        // a non-object `settings` value is the user's to own; forward verbatim
        return Some(settings);
    };

    let mut forwarded = zed::serde_json::Map::new();
    let mut dotted = Vec::new();
    for (key, value) in object {
        if is_extension_key(&key) {
            continue;
        }
        if key.contains('.') {
            dotted.push((key, value));
        } else {
            forwarded.insert(key, value);
        }
    }
    for (key, value) in dotted {
        merge_dotted_key(&mut forwarded, &key, value);
    }

    merge_user_value_into(
        zed::serde_json::Value::Object(forwarded),
        &mut configuration,
    );
    Some(configuration)
}

/// Deep-merges `user` into `target`: objects recurse, anything else (scalars,
/// arrays, null) is overwritten by the user's value.
fn merge_user_value_into(user: zed::serde_json::Value, target: &mut zed::serde_json::Value) {
    match (user, target) {
        (zed::serde_json::Value::Object(user), zed::serde_json::Value::Object(target)) => {
            for (key, value) in user {
                match target.get_mut(&key) {
                    Some(existing) => merge_user_value_into(value, existing),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (user, target) => *target = user,
    }
}

fn is_extension_key(key: &str) -> bool {
    let first = key.split('.').next().unwrap_or(key);
    EXTENSION_KEY_PREFIXES.contains(&first)
}

/// Expands a dotted key into a nested object path and merges it into `target`
/// without overwriting values that are already present (nested form wins).
fn merge_dotted_key(
    target: &mut zed::serde_json::Map<String, zed::serde_json::Value>,
    dotted_key: &str,
    value: zed::serde_json::Value,
) {
    let mut parts = dotted_key.split('.').peekable();
    let mut current = target;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            current.entry(part.to_string()).or_insert(value);
            return;
        }
        let entry = current
            .entry(part.to_string())
            .or_insert_with(|| zed::serde_json::Value::Object(zed::serde_json::Map::new()));
        match entry.as_object_mut() {
            Some(object) => current = object,
            // nested form set this path to a non-object; nested wins, drop the dotted value
            None => return,
        }
    }
}

/// Validates a `GOMEMLIMIT` value per the Go runtime's rules: an integer byte
/// count with an optional `B`/`KiB`/`MiB`/`GiB`/`TiB` suffix, or `off`.
/// (Decimal fractions and SI suffixes like `MB` crash the server at startup.)
pub fn ensure_go_mem_limit(value: &str) -> Result<()> {
    if value == "off" {
        return Ok(());
    }

    let digits_end = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(digits_end);

    if number.is_empty() {
        return Err(format!(
            "invalid GOMEMLIMIT value `{value}`: expected an integer byte count with an optional B/KiB/MiB/GiB/TiB suffix, or `off`"
        ));
    }

    match suffix {
        "" | "B" | "KiB" | "MiB" | "GiB" | "TiB" => Ok(()),
        _ => Err(format!(
            "invalid GOMEMLIMIT value `{value}`: the Go runtime only accepts B/KiB/MiB/GiB/TiB suffixes"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zed_extension_api::serde_json::{Value, json};

    fn config(value: Value) -> Option<Value> {
        workspace_configuration(Some(value))
    }

    /// Convenience: value at a `/`-separated pointer in the configuration.
    fn at(config: &Option<Value>, pointer: &str) -> Value {
        config
            .as_ref()
            .and_then(|value| value.pointer(pointer))
            .cloned()
            .unwrap_or(Value::Null)
    }

    #[test]
    fn test_workspace_configuration_defaults_when_unset() {
        let defaults = workspace_configuration(None);
        assert_eq!(
            at(&defaults, "/typescript/inlayHints/parameterNames/enabled"),
            json!("all")
        );
        assert_eq!(
            at(&defaults, "/javascript/referencesCodeLens/enabled"),
            json!(true)
        );
    }

    #[test]
    fn test_workspace_configuration_strips_extension_settings() {
        assert_eq!(
            config(json!({
                "version": "7.0.2",
                "updateChannel": "latest",
                "tsdk": {"path": "./node_modules/typescript"},
                "tsdk.path": "./node_modules/typescript",
                "server": {"pprofDir": "./pprof"},
                "server.goMemLimit": "2048MiB",
            })),
            workspace_configuration(None)
        );
    }

    #[test]
    fn test_workspace_configuration_merges_user_over_defaults() {
        let merged = config(json!({
            "typescript": {
                "inlayHints": {"parameterNames": {"enabled": "none"}},
                "preferences": {"quoteStyle": "single"},
            },
            "js/ts": {"implicitProjectConfig": {"checkJs": true}},
        }));
        // user leaf wins
        assert_eq!(
            at(&merged, "/typescript/inlayHints/parameterNames/enabled"),
            json!("none")
        );
        // sibling defaults survive
        assert_eq!(
            at(&merged, "/typescript/inlayHints/variableTypes/enabled"),
            json!(true)
        );
        // user-only values forwarded
        assert_eq!(
            at(&merged, "/typescript/preferences/quoteStyle"),
            json!("single")
        );
        assert_eq!(
            at(&merged, "/js~1ts/implicitProjectConfig/checkJs"),
            json!(true)
        );
    }

    #[test]
    fn test_workspace_configuration_expands_dotted_keys() {
        let merged = config(json!({
            "typescript.inlayHints.parameterNames.enabled": "literals",
            "typescript.preferences.quoteStyle": "single",
        }));
        assert_eq!(
            at(&merged, "/typescript/inlayHints/parameterNames/enabled"),
            json!("literals")
        );
        assert_eq!(
            at(&merged, "/typescript/preferences/quoteStyle"),
            json!("single")
        );
    }

    #[test]
    fn test_workspace_configuration_slash_section_survives_expansion() {
        let merged = config(json!({
            "js/ts.implicitProjectConfig.strictNullChecks": true,
        }));
        assert_eq!(
            at(&merged, "/js~1ts/implicitProjectConfig/strictNullChecks"),
            json!(true)
        );
    }

    #[test]
    fn test_workspace_configuration_nested_wins_over_dotted() {
        let merged = config(json!({
            "typescript": {"preferences": {"quoteStyle": "single"}},
            "typescript.preferences.quoteStyle": "double",
            "typescript.inlayHints.variableTypes.enabled": false,
        }));
        assert_eq!(
            at(&merged, "/typescript/preferences/quoteStyle"),
            json!("single")
        );
        assert_eq!(
            at(&merged, "/typescript/inlayHints/variableTypes/enabled"),
            json!(false)
        );
    }

    #[test]
    fn test_string_setting_nested_wins_and_type_errors() {
        let settings = Some(json!({
            "server": {"pprofDir": "./nested"},
            "server.pprofDir": "./dotted",
        }));
        assert_eq!(
            string_setting(&settings, ExtensionSetting::PprofDir).unwrap(),
            Some("./nested".to_string())
        );

        let settings = Some(json!({"server": {"pprofDir": 5}}));
        assert!(string_setting(&settings, ExtensionSetting::PprofDir).is_err());
    }

    #[test]
    fn test_string_array_setting() {
        let settings = Some(json!({"server": {"args": ["--pprofDir", "./p"]}}));
        assert_eq!(
            string_array_setting(&settings, ExtensionSetting::ServerArgs).unwrap(),
            Some(vec!["--pprofDir".to_string(), "./p".to_string()])
        );

        let settings = Some(json!({"server": {"args": ["ok", 5]}}));
        assert!(string_array_setting(&settings, ExtensionSetting::ServerArgs).is_err());
    }

    #[test]
    fn test_string_map_setting() {
        let settings = Some(json!({"server": {"env": {"GOGC": "50"}}}));
        assert_eq!(
            string_map_setting(&settings, ExtensionSetting::ServerEnv).unwrap(),
            Some(vec![("GOGC".to_string(), "50".to_string())])
        );

        let settings = Some(json!({"server": {"env": {"GOGC": 50}}}));
        assert!(string_map_setting(&settings, ExtensionSetting::ServerEnv).is_err());
    }

    #[test]
    fn test_go_mem_limit() {
        assert!(ensure_go_mem_limit("2048MiB").is_ok());
        assert!(ensure_go_mem_limit("1024").is_ok());
        assert!(ensure_go_mem_limit("1GiB").is_ok());
        assert!(ensure_go_mem_limit("off").is_ok());
        // the Go runtime rejects decimals and SI suffixes at startup
        assert!(ensure_go_mem_limit("1.5GiB").is_err());
        assert!(ensure_go_mem_limit("2048MB").is_err());
        assert!(ensure_go_mem_limit("foo").is_err());
        assert!(ensure_go_mem_limit(".5GiB").is_err());
        assert!(ensure_go_mem_limit("GiB").is_err());
    }
}
