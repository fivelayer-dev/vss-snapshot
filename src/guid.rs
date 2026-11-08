use std::fmt;
use std::str::FromStr;

/// A GUID, as VSS uses to identify writers, snapshots and snapshot sets.
///
/// `Display` gives the lowercase braced form that `vssadmin` prints, such as
/// `{a65faa63-5ea8-4ebc-9dbd-a0c4db26912a}`. [`FromStr`] accepts that form with or without
/// the braces, in any case.
///
/// ```
/// # #[cfg(windows)] {
/// use vss_snapshot::Guid;
///
/// let id: Guid = "A65FAA63-5EA8-4EBC-9DBD-A0C4DB26912A".parse().unwrap();
/// assert_eq!(id.to_string(), "{a65faa63-5ea8-4ebc-9dbd-a0c4db26912a}");
/// assert_eq!(id, Guid::from_u128(0xa65faa63_5ea8_4ebc_9dbd_a0c4db26912a));
/// # }
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Guid(u128);

impl Guid {
    /// Makes a GUID from its value written as one number, in the order it's printed.
    pub const fn from_u128(value: u128) -> Guid {
        Guid(value)
    }

    /// The GUID's value as one number, in the order it's printed.
    pub const fn to_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        write!(
            f,
            "{{{:08x}-{:04x}-{:04x}-{:04x}-{:012x}}}",
            v >> 96,
            (v >> 80) & 0xffff,
            (v >> 64) & 0xffff,
            (v >> 48) & 0xffff,
            v & 0xffff_ffff_ffff
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

impl FromStr for Guid {
    type Err = ParseGuidError;

    fn from_str(s: &str) -> Result<Guid, ParseGuidError> {
        let error = || ParseGuidError {
            input: s.to_owned(),
        };
        let inner = match s.strip_prefix('{') {
            Some(rest) => rest.strip_suffix('}').ok_or_else(error)?,
            None => s,
        };
        if inner.len() != 36 {
            return Err(error());
        }
        let mut value: u128 = 0;
        for (index, c) in inner.chars().enumerate() {
            if matches!(index, 8 | 13 | 18 | 23) {
                if c != '-' {
                    return Err(error());
                }
                continue;
            }
            let digit = c.to_digit(16).ok_or_else(error)?;
            value = (value << 4) | u128::from(digit);
        }
        Ok(Guid(value))
    }
}

/// Returned when a string isn't a GUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseGuidError {
    input: String,
}

impl fmt::Display for ParseGuidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a GUID: {:?}", self.input)
    }
}

impl std::error::Error for ParseGuidError {}

#[cfg(test)]
mod tests {
    use super::*;

    const SQL_WRITER: u128 = 0xa65faa63_5ea8_4ebc_9dbd_a0c4db26912a;

    #[test]
    fn prints_like_vssadmin() {
        assert_eq!(
            Guid::from_u128(SQL_WRITER).to_string(),
            "{a65faa63-5ea8-4ebc-9dbd-a0c4db26912a}"
        );
        assert_eq!(
            Guid::default().to_string(),
            "{00000000-0000-0000-0000-000000000000}"
        );
        assert_eq!(
            Guid::from_u128(u128::MAX).to_string(),
            "{ffffffff-ffff-ffff-ffff-ffffffffffff}"
        );
    }

    #[test]
    fn parses_with_or_without_braces_in_any_case() {
        for text in [
            "{a65faa63-5ea8-4ebc-9dbd-a0c4db26912a}",
            "a65faa63-5ea8-4ebc-9dbd-a0c4db26912a",
            "{A65FAA63-5EA8-4EBC-9DBD-A0C4DB26912A}",
        ] {
            assert_eq!(
                text.parse::<Guid>().unwrap().to_u128(),
                SQL_WRITER,
                "{text}"
            );
        }
    }

    #[test]
    fn round_trips() {
        let id = Guid::from_u128(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
        assert_eq!(id.to_string().parse::<Guid>().unwrap(), id);
    }

    #[test]
    fn rejects_anything_else() {
        for text in [
            "",
            "{}",
            "{a65faa63-5ea8-4ebc-9dbd-a0c4db26912a",
            "a65faa63-5ea8-4ebc-9dbd-a0c4db26912a}",
            "a65faa63-5ea8-4ebc-9dbd-a0c4db26912",
            "a65faa63-5ea8-4ebc-9dbd-a0c4db26912aa",
            "a65faa635ea8-4ebc-9dbd-a0c4db26912aa",
            "g65faa63-5ea8-4ebc-9dbd-a0c4db26912a",
            "a65faa63-5ea8-4ebc-9dbd-a0c4db2691é",
            "+65faa63-5ea8-4ebc-9dbd-a0c4db26912a",
        ] {
            assert!(text.parse::<Guid>().is_err(), "{text}");
        }
    }

    #[test]
    fn the_error_names_the_input() {
        let error = "nope".parse::<Guid>().unwrap_err();
        assert_eq!(error.to_string(), r#"not a GUID: "nope""#);
    }
}
