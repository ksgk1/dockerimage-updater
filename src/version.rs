use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::docker_file::{Error, ParseError};

#[derive(Debug, Clone, Default, Eq, PartialEq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct TagVariant {
    pub name:              Option<String>,
    pub major:             Option<u64>,
    pub minor:             Option<u64>,
    pub patch:             Option<u64>,
    pub version_delimiter: Option<String>,
}

impl Display for TagVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{name}")?,
            None => write!(f, "")?,
        }
        match self.major {
            Some(major) => write!(f, "{major}")?,
            None => write!(f, "")?,
        }
        match &self.version_delimiter {
            Some(delimiter) => write!(f, "{delimiter}")?,
            None => {
                if self.minor.is_some() {
                    write!(f, ".")?;
                } else {
                    write!(f, "")?;
                }
            }
        }
        match self.minor {
            Some(minor) => write!(f, "{minor}")?,
            None => write!(f, "")?,
        }
        match self.patch {
            Some(patch) => write!(f, ".{patch}"),
            None => write!(f, ""),
        }
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct Tag {
    pub major:           Option<u64>,
    pub minor:           Option<u64>,
    pub patch:           Option<u64>,
    pub variant:         Option<TagVariant>,
    pub prefix:          Option<char>,
    pub allowed_missing: bool,
}

impl Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.prefix.is_some() {
            write!(f, "{}", self.prefix.expect("Prefix was set."))?;
        }
        match self.major {
            Some(major) => write!(f, "{major}")?,
            None => write!(f, "")?,
        }
        match self.minor {
            Some(minor) => write!(f, ".{minor}")?,
            None => write!(f, "")?,
        }
        match self.patch {
            Some(patch) => write!(f, ".{patch}")?,
            None => write!(f, "")?,
        }
        match &self.variant {
            Some(variant) => {
                if self.major.is_some() {
                    write!(f, "-")?;
                }
                write!(f, "{variant}")
            }
            None => write!(f, ""),
        }
    }
}

impl FromStr for Tag {
    type Err = Error;

    #[allow(clippy::too_many_lines)]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn extract_string_between_numbers(input: &str) -> Option<String> {
            let mut in_number = false;
            let mut start = None;
            let mut result = String::new();

            for (i, c) in input.chars().enumerate() {
                if c.is_ascii_digit() {
                    if !in_number {
                        in_number = true;
                        if start.is_some() {
                            return Some(result); // Break out when we find the second digit
                        }
                    }
                } else if in_number {
                    if start.is_none() {
                        start = Some(i); // Mark where non-numeric chars start
                    }
                    result.push(c);
                }
            }

            if result.is_empty() { None } else { Some(result) }
        }

        #[allow(clippy::too_many_lines)]
        fn parse_version_parts(version: &str) -> (Option<u64>, Option<u64>, Option<u64>, Option<String>) {
            let mut parts: Vec<&str> = version.split('.').collect();
            #[allow(unused_assignments)]
            let mut new_parts_buffer = Vec::<String>::new(); // buffer needed here to live long enough
            let mut delim = None;
            if parts.len() == 1 {
                // If the split was unable to split, we need to check if it returned the
                // original string
                let de = extract_string_between_numbers(parts.first().expect("At least one element exists")).unwrap_or_else(|| "-".to_owned());
                delim = Some(de.clone());
                parts = version.split(&de).collect();
                new_parts_buffer = parts
                    .iter()
                    .map(|part| part.chars().filter(|c| !c.is_alphabetic()).collect::<String>())
                    .collect();
                parts = new_parts_buffer.iter().map(String::as_str).collect();
            }
            let major = parts.first().and_then(|v| v.parse::<u64>().ok());
            let minor = parts.get(1).and_then(|v| v.parse::<u64>().ok());
            let patch = parts.get(2).and_then(|v| v.parse::<u64>().ok());
            (major, minor, patch, delim)
        }

        if s.trim().is_empty() {
            return Err(Error::Parse(ParseError::InvalidTag(String::new())));
        }

        if s.trim().eq_ignore_ascii_case("latest") {
            // latest is a valid  tag, we can not reflect it in the tag, but will handle the
            // latest case in `ImageMetadata`
            return Ok(Self::default());
        }

        if let Some((version, variant_str)) = s.split_once('-') {
            let prefix = if version.to_ascii_lowercase().starts_with('v') {
                let p = version.to_string().chars().next().expect("Version is not empty.");
                Some(p)
            } else {
                None
            };
            let (major, minor, patch, _) = if prefix.is_some() {
                // if we have a prefix, we ignore the prefix
                parse_version_parts(&version[1..])
            } else {
                parse_version_parts(version)
            };
            // Variant
            let mut chars = variant_str.chars();
            let name_end = chars.position(|c| c.is_ascii_digit()).unwrap_or(variant_str.len());
            let variant_name = Some(variant_str[..name_end].to_owned());
            let mut variant_major = None;
            let mut variant_minor = None;
            let mut variant_patch = None;
            let mut delim = None;

            if name_end < variant_str.len() {
                let version_part = &variant_str[name_end..];
                (variant_major, variant_minor, variant_patch, delim) = parse_version_parts(version_part);
            }

            let variant = Some(TagVariant {
                name:              variant_name,
                major:             variant_major,
                minor:             variant_minor,
                patch:             variant_patch,
                version_delimiter: delim,
            });
            Ok(Self {
                major,
                minor,
                patch,
                variant,
                prefix,
                allowed_missing: false,
            })
        } else {
            let prefix = if s.to_ascii_lowercase().starts_with('v') {
                let p = s.to_string().chars().next().expect("Version is not empty.");
                Some(p)
            } else {
                None
            };
            let (major, minor, patch, _) = if prefix.is_some() {
                // if we have a prefix, we ignore the prefix
                parse_version_parts(&s[1..])
            } else {
                parse_version_parts(s)
            };
            if major.is_none() {
                return Ok(Self {
                    major,
                    minor,
                    patch,
                    variant: Some(TagVariant {
                        name:              Some(s.to_owned()),
                        major:             None,
                        minor:             None,
                        patch:             None,
                        version_delimiter: None,
                    }),
                    prefix,
                    allowed_missing: false,
                });
            }
            Ok(Self {
                major,
                minor,
                patch,
                variant: None,
                prefix,
                allowed_missing: false,
            })
        }
    }
}

impl AsRef<Self> for Tag {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl Tag {
    pub fn to_key_value_pair(&self) -> Option<(String, String)> {
        let mut key = String::new();
        if let Some(major) = self.major {
            key.push_str(&major.to_string());
        }
        if let Some(minor) = self.minor {
            key.push('.');
            key.push_str(&minor.to_string());
        }
        if let Some(patch) = self.patch {
            key.push('.');
            key.push_str(&patch.to_string());
        }

        if let Some(variant) = &self.variant
            && let Some(name) = &variant.name
        {
            key.push('-');
            key.push_str(name);
        }

        let mut value = String::new();
        if let Some(variant) = &self.variant {
            if let Some(major) = variant.major {
                value.push_str(&major.to_string());
            }
            match &variant.version_delimiter {
                Some(del) => value.push_str(del),
                None => value.push('.'),
            }
            if let Some(minor) = variant.minor {
                value.push_str(&minor.to_string());
            }
            if let Some(patch) = variant.patch {
                value.push('.');
                value.push_str(&patch.to_string());
            }
        }

        if key.is_empty() || value.is_empty() { None } else { Some((key, value)) }
    }
}

#[derive(Debug)]
pub struct VersionTags {
    pub(crate) tags: Vec<Tag>,
}

impl Display for VersionTags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for version in &self.tags {
            writeln!(f, "{version}")?;
        }
        write!(f, "")
    }
}

/// # Returns
///
/// * `true`: Only if the current major version is the same and the minor
///   version of the next tag is greater than current. Or of the patch is
///   greater while the minor version is the same. If a variant is given, it
///   will also check if the variant is the same for both tags.
/// * `false`: Otherwise
pub fn is_next_minor(current_tag: &Tag, next_tag: &Tag) -> bool {
    match (
        current_tag.major,
        current_tag.minor,
        current_tag.patch,
        current_tag.variant.as_ref(),
        next_tag.major,
        next_tag.minor,
        next_tag.patch,
        next_tag.variant.as_ref(),
    ) {
        (Some(current_major), Some(current_minor), None, None, Some(next_major), Some(next_minor), None, None) => {
            current_major == next_major && current_minor < next_minor
        }
        (Some(current_major), Some(current_minor), Some(current_patch), None, Some(next_major), Some(next_minor), Some(next_patch), None) => {
            ((current_major == next_major) && (current_minor < next_minor))
                || ((current_major == next_major) && (current_minor == next_minor && current_patch < next_patch))
        }
        (Some(current_major), Some(current_minor), None, Some(current_variant), Some(next_major), Some(next_minor), None, Some(next_variant)) => {
            current_major == next_major && current_minor < next_minor && current_variant.name == next_variant.name
        }
        (
            Some(current_major),
            Some(current_minor),
            Some(current_patch),
            Some(current_variant),
            Some(next_major),
            Some(next_minor),
            Some(next_patch),
            Some(next_variant),
        ) => {
            ((current_major == next_major) && (current_minor < next_minor) && (current_variant.name == next_variant.name))
                || ((current_major == next_major) && (current_minor == next_minor && current_patch < next_patch) && (current_variant.name == next_variant.name))
                || ((current_major == next_major)
                    && (current_minor == next_minor)
                    && (current_patch == next_patch)
                    && (current_variant.name == next_variant.name)
                    && ((current_variant.major.is_some() && next_variant.major.is_some()) && current_variant.major > next_variant.major
                        || ((current_variant.major.is_some()
                            && next_variant.major.is_some()
                            && current_variant.minor.is_some()
                            && next_variant.minor.is_some())
                            && (current_variant.major < next_variant.major && current_variant.minor == next_variant.minor)
                            || (current_variant.major <= next_variant.major && current_variant.minor < next_variant.minor))))
        }
        _ => false,
    }
}

pub fn is_next_major(current_tag: &Tag, next_tag: &Tag) -> bool {
    match (
        current_tag.major,
        current_tag.minor,
        current_tag.patch,
        current_tag.variant.as_ref(),
        next_tag.major,
        next_tag.minor,
        next_tag.patch,
        next_tag.variant.as_ref(),
    ) {
        (Some(current_major), Some(_), Some(_), None, Some(next_major), Some(_), Some(_), None)
        | (Some(current_major), Some(_), None, None, Some(next_major), Some(_), None, None) => current_major < next_major,
        (Some(current_major), Some(_), None, Some(current_variant), Some(next_major), Some(_), None, Some(next_variant))
        | (Some(current_major), Some(_), Some(_), Some(current_variant), Some(next_major), Some(_), Some(_), Some(next_variant)) => {
            (current_major < next_major) && (current_variant.name == next_variant.name)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use pretty_assertions::assert_eq;

    use crate::Tag;
    use crate::version::{TagVariant, VersionTags, is_next_major, is_next_minor};

    #[test]
    fn parsing() {
        let expected = String::from("a");
        let tag: Result<Tag, _> = expected.parse();
        assert!(tag.is_ok());
        let actual = tag.unwrap().variant.unwrap().name.unwrap();
        assert_eq!(expected, actual);

        let tag: Tag = "v24".parse().unwrap();
        assert_eq!(tag.major, Some(24));
        assert_eq!(tag.prefix, Some('v'));

        let tag: Tag = "V0".parse().unwrap();
        assert_eq!(tag.major, Some(0));
        assert_eq!(tag.prefix, Some('V'));

        let tag: Tag = "24".parse().unwrap();
        assert_eq!(tag.major, Some(24));

        let s = "24.0.0-alpine3.22";
        let tag: Tag = s.parse().unwrap();
        assert_eq!(tag.major, Some(24));
        assert_eq!(tag.minor, Some(0));
        assert_eq!(tag.patch, Some(0));
        assert_eq!(
            tag.variant,
            Some(TagVariant {
                name:              Some("alpine".to_owned()),
                version_delimiter: None,
                major:             Some(3),
                minor:             Some(22),
                patch:             None,
            })
        );
        let s2 = tag.to_string();
        assert_eq!(s2, s.to_owned());

        let s = "24.0-alpine3.21.1";
        let tag: Tag = s.parse().unwrap();
        assert_eq!(tag.major, Some(24));
        assert_eq!(tag.minor, Some(0));
        assert_eq!(tag.patch, None);
        assert_eq!(
            tag.variant,
            Some(TagVariant {
                name:              Some("alpine".to_owned()),
                version_delimiter: None,
                major:             Some(3),
                minor:             Some(21),
                patch:             Some(1),
            })
        );
        let s2 = tag.to_string();
        assert_eq!(s2, s.to_owned());

        let empty_tag = Tag::default();
        let s = "";
        assert_eq!(empty_tag.to_string(), s);
        let s = "9.1.1-debian-13-r8";
        let v: Tag = s.parse().unwrap();
        assert_eq!(v.variant.clone().unwrap().version_delimiter, Some("-r".to_owned()));
        let s2 = v.to_string();
        assert_eq!(s2, s.to_owned());
        let b = v.to_key_value_pair().unwrap();
        assert_eq!(b.0, "9.1.1-debian-".to_owned());
        assert_eq!(b.1, "13-r8".to_owned());
        let vtags = VersionTags { tags: vec![v] };
        assert_eq!(vtags.to_string(), "9.1.1-debian-13-r8\n".to_owned());
    }

    #[test]
    fn next_minor() {
        let cases = [
            ("2.5", "2.6", true),
            ("2.5.7", "2.6.9", true),
            ("2.6.9", "2.6.10", true),
            ("2.6.9-bookworm-slim", "2.6.10-bookworm-slim", true),
            ("9.0.1-debian-12-r8", "9.0.1-debian-12-r9", true),
            ("9.0.1-debian-12-r8", "9.0.1-debian-13-r8", true),
            ("9.0-debian-12-r8", "9.1-debian-13-r8", true),
            ("2.6.9", "2.6.9", false),
            ("2.6.9", "3.6.10", false),
            ("2.6.9-bookworm-slim", "3.6.10-bookworm-slim", false),
            ("2.6.9-bookworm-slim", "2.6.8-bookworm-slim", false),
            ("2.6.9-bookworm-slim", "2.6.10-bookwork-slim", false),
        ];

        for (current, next, expect) in &cases {
            let c = current.parse::<Tag>().expect("left tag valid");
            let n = next.parse::<Tag>().expect("right tag valid");
            let got = is_next_minor(&c, &n);
            assert_eq!(got, *expect, "is_next_minor({}, {}) → expected {}, got {}", current, next, expect, got);
        }
    }

    #[test]
    fn next_major() {
        let cases = [
            ("2.5.7", "3.0.0", true),
            ("2.6.9-bookworm-slim", "3.6.10-bookworm-slim", true),
            ("8.0.1-debian-12-r8", "9.0.1-debian-13-r8", true),
            ("8.0.1-debian-12-r8", "9.0.1-debian-12-r8", true),
            ("8.0-debian-12-r8", "9.0-debian-12-r8", true),
            ("2.6.9", "2.7.9", false),
        ];

        for (current, next, expect) in &cases {
            let c = current.parse::<Tag>().expect("left tag valid");
            let n = next.parse::<Tag>().expect("right tag valid");
            let got = is_next_major(&c, &n);
            assert_eq!(got, *expect, "is_next_major({}, {}) → expected {}, got {}", current, next, expect, got);
        }
    }
}
