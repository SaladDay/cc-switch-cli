//! Portable identity for one managed Skill directory.
//!
//! A Skill directory is data received from databases, deep links, and remote
//! bundles, but it later becomes a filesystem component. Keep that conversion
//! behind one platform-independent validator so Unix never accepts a name that
//! would escape, alias, or become a device path on Windows.

use std::fmt;

use unicode_normalization::UnicodeNormalization;

const MAX_COMPONENT_BYTES: usize = 255;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SkillDirectory(String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InvalidPortableComponent {
    reason: &'static str,
}

impl InvalidPortableComponent {
    pub(crate) fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for InvalidPortableComponent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason)
    }
}

impl std::error::Error for InvalidPortableComponent {}

impl SkillDirectory {
    pub(crate) fn parse(value: &str) -> Result<Self, InvalidPortableComponent> {
        validate_portable_component(value)?;
        Ok(Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Filesystems used by supported platforms disagree about Unicode and
    /// case. This key deliberately adopts the stricter shared identity so two
    /// database rows can never address one physical directory.
    pub(crate) fn collision_key(&self) -> String {
        self.0.nfkc().flat_map(char::to_lowercase).nfkc().collect()
    }
}

pub(crate) fn validate_portable_component(value: &str) -> Result<(), InvalidPortableComponent> {
    validate_component_shape(value)?;
    let compatibility_normalized = value.nfkc().collect::<String>();
    validate_component_shape(&compatibility_normalized)
}

fn validate_component_shape(value: &str) -> Result<(), InvalidPortableComponent> {
    if value.is_empty() {
        return Err(invalid("directory is empty"));
    }
    if value.len() > MAX_COMPONENT_BYTES {
        return Err(invalid("directory exceeds the portable component limit"));
    }
    if value == "." || value == ".." {
        return Err(invalid("dot path components are not allowed"));
    }
    if value.ends_with(['.', ' ']) {
        return Err(invalid(
            "trailing dots and spaces are not portable directory names",
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid("control characters are not allowed"));
    }
    if value.chars().any(|character| {
        matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        )
    }) {
        return Err(invalid(
            "path separators and Windows-special characters are not allowed",
        ));
    }

    let device_stem = value
        .split_once('.')
        .map_or(value, |(stem, _extension)| stem)
        .to_ascii_uppercase();
    if is_windows_reserved_name(&device_stem) {
        return Err(invalid("Windows reserved device names are not allowed"));
    }
    Ok(())
}

fn is_windows_reserved_name(value: &str) -> bool {
    matches!(
        value,
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || value
        .strip_prefix("COM")
        .or_else(|| value.strip_prefix("LPT"))
        .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

const fn invalid(reason: &'static str) -> InvalidPortableComponent {
    InvalidPortableComponent { reason }
}

#[cfg(test)]
mod tests {
    use super::SkillDirectory;

    #[test]
    fn collision_key_is_case_and_unicode_normalization_independent() {
        let composed = SkillDirectory::parse("Résumé").expect("valid composed name");
        let decomposed =
            SkillDirectory::parse("Re\u{301}sume\u{301}").expect("valid decomposed name");
        let uppercase = SkillDirectory::parse("RÉSUMÉ").expect("valid uppercase name");

        assert_eq!(composed.collision_key(), decomposed.collision_key());
        assert_eq!(composed.collision_key(), uppercase.collision_key());
    }

    #[test]
    fn compatibility_equivalents_cannot_smuggle_special_characters() {
        assert!(SkillDirectory::parse("skill\u{ff1a}name").is_err());
        assert!(SkillDirectory::parse("CON").is_err());
        assert!(SkillDirectory::parse("ＣＯＮ").is_err());
    }
}
