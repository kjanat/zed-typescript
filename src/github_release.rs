use zed_extension_api::{self as zed, Result};

const PAGE_SIZE: usize = 100;

/// Select a compiler release, excluding other products published in the repository.
pub fn latest(include_prereleases: bool) -> Result<zed::GithubRelease> {
    latest_with(include_prereleases, |page| {
        let response = zed::http_client::HttpRequest::builder()
            .method(zed::http_client::HttpMethod::Get)
            .url(format!(
                "https://api.github.com/repos/microsoft/TypeScript/releases?per_page={PAGE_SIZE}&page={page}"
            ))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "zed-typescript")
            .redirect_policy(zed::http_client::RedirectPolicy::FollowLimit(3))
            .build()?
            .fetch()?;
        Ok(response.body)
    })
}

fn latest_with(
    include_prereleases: bool,
    mut fetch_page: impl FnMut(u32) -> Result<Vec<u8>>,
) -> Result<zed::GithubRelease> {
    for page in 1..=u32::MAX {
        let body = fetch_page(page)?;
        let value: zed::serde_json::Value = zed::serde_json::from_slice(&body)
            .map_err(|_| "Invalid GitHub TypeScript release response".to_string())?;
        let releases = value
            .as_array()
            .ok_or_else(|| "Expected a list of GitHub TypeScript releases".to_string())?;
        for release in releases {
            if let Some(release) = compiler_release(release, include_prereleases)? {
                return Ok(release);
            }
        }
        if releases.len() < PAGE_SIZE {
            break;
        }
    }
    Err("No eligible TypeScript 7+ compiler release found on GitHub".into())
}

fn compiler_release(
    release: &zed::serde_json::Value,
    include_prereleases: bool,
) -> Result<Option<zed::GithubRelease>> {
    let malformed = || "Malformed GitHub TypeScript release metadata".to_string();
    let tag = release["tag_name"].as_str().ok_or_else(malformed)?;
    let draft = release["draft"].as_bool().ok_or_else(malformed)?;
    let prerelease = release["prerelease"].as_bool().ok_or_else(malformed)?;
    let Ok(version) = semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag)) else {
        return Ok(None);
    };
    if draft
        || version.major < 7
        || (!include_prereleases && (prerelease || !version.pre.is_empty()))
    {
        return Ok(None);
    }
    let assets = release["assets"].as_array().ok_or_else(malformed)?;
    let assets = assets
        .iter()
        .map(|asset| {
            Ok(zed::GithubReleaseAsset {
                name: asset["name"].as_str().ok_or_else(malformed)?.to_string(),
                download_url: asset["browser_download_url"]
                    .as_str()
                    .ok_or_else(malformed)?
                    .to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if !assets.iter().any(|asset| {
        asset.name == "typescript.tgz"
            || asset
                .name
                .strip_prefix("typescript-")
                .and_then(|name| name.strip_suffix(".tgz"))
                .is_some_and(|platform| !platform.is_empty())
    }) {
        return Ok(None);
    }
    Ok(Some(zed::GithubRelease {
        version: tag.to_string(),
        assets,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zed::serde_json::{Value, json};

    fn release(tag: &str, prerelease: bool, draft: bool, asset: &str) -> Value {
        json!({
            "tag_name": tag,
            "prerelease": prerelease,
            "draft": draft,
            "assets": [{
                "name": asset,
                "browser_download_url": "https://github.com/microsoft/TypeScript/releases/download/test/archive.tgz"
            }]
        })
    }

    fn page(releases: Vec<Value>) -> Vec<u8> {
        zed::serde_json::to_vec(&releases).unwrap()
    }

    #[test]
    fn channels_skip_other_products_old_versions_and_drafts() {
        let releases = page(vec![
            release("vscode-typescript/v1.0.1", false, false, "extension.vsix"),
            release("v6.0.2", false, false, "typescript.tgz"),
            release("v8.0.0", false, true, "typescript.tgz"),
            release("v7.1.0-beta.1", true, false, "typescript.tgz"),
            release("v7.0.2", false, false, "typescript-win32-x64.tgz"),
        ]);
        for (include_prereleases, expected) in [(false, "v7.0.2"), (true, "v7.1.0-beta.1")] {
            let selected = latest_with(include_prereleases, |_| Ok(releases.clone())).unwrap();
            assert_eq!(selected.version, expected);
        }
        let stable = page(vec![release("v7.0.2", false, false, "typescript.tgz")]);
        assert_eq!(
            latest_with(true, |_| Ok(stable.clone())).unwrap().version,
            "v7.0.2"
        );
    }

    #[test]
    fn stable_channel_checks_both_prerelease_indicators() {
        let releases = page(vec![
            release("v7.2.0", true, false, "typescript.tgz"),
            release("v7.1.0-beta.1", false, false, "typescript.tgz"),
            release("v7.0.2", false, false, "typescript.tgz"),
        ]);
        assert_eq!(
            latest_with(false, |_| Ok(releases.clone()))
                .unwrap()
                .version,
            "v7.0.2"
        );
    }

    #[test]
    fn unsupported_tags_and_noncompiler_assets_are_skipped() {
        let releases = page(vec![
            release("v7.1-beta", true, false, "typescript.tgz"),
            release("garbage", false, false, "typescript.tgz"),
            release("v7.3.0", false, false, "extension.vsix"),
            release("v7.2.0", false, false, "typescript-.tgz"),
            release("v7.0.2", false, false, "typescript-linux-arm64.tgz"),
        ]);
        // Selection is platform-independent; missing platform assets must not
        // silently select an older compiler version.
        let selected = latest_with(true, |_| Ok(releases.clone())).unwrap();
        assert_eq!(selected.version, "v7.0.2");
        assert_eq!(selected.assets[0].name, "typescript-linux-arm64.tgz");
    }

    #[test]
    fn scans_past_full_unrelated_pages_and_stops_on_exhaustion() {
        let unrelated = release("vscode-typescript/v1.0.1", false, false, "extension.vsix");
        let mut requests = Vec::new();
        let selected = latest_with(false, |number| {
            requests.push(number);
            Ok(if number == 1 {
                page(vec![unrelated.clone(); PAGE_SIZE])
            } else {
                page(vec![release("v7.0.2", false, false, "typescript.tgz")])
            })
        })
        .unwrap();
        assert_eq!(selected.version, "v7.0.2");
        assert_eq!(requests, vec![1, 2]);

        for final_page in [vec![], vec![unrelated.clone()]] {
            requests.clear();
            assert!(
                latest_with(false, |number| {
                    requests.push(number);
                    Ok(page(if number == 1 {
                        vec![unrelated.clone(); PAGE_SIZE]
                    } else {
                        final_page.clone()
                    }))
                })
                .is_err()
            );
            assert_eq!(requests, vec![1, 2]);
        }
    }

    #[test]
    fn request_failures_and_malformed_responses_remain_errors() {
        let error = latest_with(false, |number| {
            if number == 1 {
                Ok(page(vec![
                    release("v6.0.2", false, false, "typescript.tgz");
                    PAGE_SIZE
                ]))
            } else {
                Err("GitHub unavailable".into())
            }
        });
        assert_eq!(error.err().as_deref(), Some("GitHub unavailable"));
        for response in [
            b"not json".to_vec(),
            br#"{"message":"API rate limit exceeded"}"#.to_vec(),
            b"[{}]".to_vec(),
            br#"[{"tag_name":"v7.0.2","draft":false,"prerelease":false,"assets":null}]"#.to_vec(),
        ] {
            assert!(latest_with(false, |_| Ok(response.clone())).is_err());
        }
    }
}
