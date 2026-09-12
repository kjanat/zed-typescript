use zed_extension_api::Result;

/// Host platform identifiers used to select a TypeScript release asset.
/// Keep this independent of Zed's narrower OS and architecture enums.
#[derive(Debug, PartialEq, Eq)]
pub struct Platform {
    pub name: String,
    pub executable: &'static str,
}

impl Platform {
    pub fn new(os: &str, arch: &str) -> Result<Self> {
        // These names also become cache path components. Unsupported but valid
        // names are handled by the release's actual asset list.
        let valid = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        };
        if !valid(os) || !valid(arch) {
            return Err("Invalid OS or architecture returned by the host Node runtime".into());
        }
        Ok(Self {
            name: format!("{os}-{arch}"),
            executable: if os == "win32" { "tsc.exe" } else { "tsc" },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names_include_freebsd_and_other_release_targets() {
        for (os, architectures) in [
            ("freebsd", &["x64", "arm64"][..]),
            ("darwin", &["x64", "arm64"][..]),
            ("win32", &["x64", "arm64"][..]),
            (
                "linux",
                &[
                    "x64", "arm64", "arm", "riscv64", "loong64", "ppc64", "s390x", "mips64el",
                ][..],
            ),
            ("netbsd", &["x64", "arm64"][..]),
            ("openbsd", &["x64", "arm64"][..]),
            ("aix", &["ppc64"][..]),
            ("sunos", &["x64"][..]),
        ] {
            for arch in architectures {
                let platform = Platform::new(os, arch).unwrap();
                assert_eq!(platform.name, format!("{os}-{arch}"));
                assert_eq!(
                    platform.executable,
                    if os == "win32" { "tsc.exe" } else { "tsc" }
                );
            }
        }
    }

    #[test]
    fn host_identifiers_cannot_escape_the_cache_directory() {
        for (os, arch) in [
            ("", "x64"),
            ("freebsd", "../x64"),
            ("../linux", "x64"),
            ("win32", "x64\\other"),
        ] {
            assert!(Platform::new(os, arch).is_err());
        }
    }
}
