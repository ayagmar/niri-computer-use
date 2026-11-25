//! A desk's name: 1 to 16 bytes of `[a-z0-9-]`, not starting with `-`, so it names one
//! directory under the desks' root and can't be read as an option.

use std::fmt;

/// The longest name, in bytes. With it the longest socket a desk holds stays within a
/// Unix socket's 107 bytes (see `layout::fits`).
pub(crate) const MAX: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct DeskName(String);

impl DeskName {
    pub(crate) fn parse(name: &str) -> Result<Self, String> {
        if name.is_empty() || name.len() > MAX {
            return Err(format!(
                "a desk's name is 1 to {MAX} bytes of a-z, 0-9 and -, not {name:?}"
            ));
        }
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(format!(
                "a desk's name holds only a-z, 0-9 and -, not {name:?}"
            ));
        }
        if name.starts_with('-') {
            return Err(format!(
                "a desk's name can't start with -, which reads as an option: {name:?}"
            ));
        }
        Ok(Self(name.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeskName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for DeskName {
    type Error = String;

    fn try_from(name: String) -> Result<Self, String> {
        Self::parse(&name)
    }
}

impl From<DeskName> for String {
    fn from(name: DeskName) -> Self {
        name.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_names_one_directory_and_never_an_option() {
        for good in ["a", "web", "research-2", "0", "abcdefghijklmnop"] {
            assert_eq!(DeskName::parse(good).unwrap().as_str(), good);
        }
        for bad in [
            "",
            "abcdefghijklmnopq",
            "Web",
            "a/b",
            "..",
            ".",
            "a b",
            "-x",
            "--windowed",
            "é",
            "a_b",
        ] {
            assert!(DeskName::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_recorded_name_is_checked_when_read_back() {
        assert!(serde_json::from_str::<DeskName>("\"../host\"").is_err());
        let name: DeskName = serde_json::from_str("\"web\"").unwrap();
        assert_eq!(serde_json::to_string(&name).unwrap(), "\"web\"");
    }
}
