use super::DomainError;

const MAX_ID_LEN: usize = 32;
const MAX_NAME_LEN: usize = 64;

pub(super) fn validate_name(name: &str) -> Result<&str, DomainError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN || name.chars().any(char::is_control)
    {
        Err(DomainError::InvalidName)
    } else {
        Ok(name)
    }
}

pub(super) fn is_valid_id(s: &str) -> bool {
    (1..=MAX_ID_LEN).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// `"Living Lamp"` -> `"living-lamp"`; everything that is not an ASCII letter or digit becomes a single dash.
pub(super) fn slugify(name: &str) -> Result<String, DomainError> {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(MAX_ID_LEN);
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        Err(DomainError::EmptySlug)
    } else {
        Ok(slug.to_owned())
    }
}

/// Defines a validated, serde-transparent string id (`DeviceId`, `ScenarioId`).
macro_rules! slug_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn parse(s: &str) -> Result<Self, $crate::domain::DomainError> {
                if $crate::domain::naming::is_valid_id(s) {
                    Ok(Self(s.to_owned()))
                } else {
                    Err($crate::domain::DomainError::InvalidId(s.to_owned()))
                }
            }

            pub fn from_name(name: &str) -> Result<Self, $crate::domain::DomainError> {
                $crate::domain::naming::slugify(name).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::domain::DomainError;

            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::parse(&s)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

pub(super) use slug_id;

#[cfg(test)]
mod tests;
