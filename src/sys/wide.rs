//! Conversions between Rust strings and the UTF-16 strings Windows uses.

/// `text` as a nul-terminated UTF-16 string.
pub(crate) fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The text in `buffer` up to its first nul, or all of it if there's none. Unpaired
/// surrogates become U+FFFD.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let text = buffer.split(|&unit| unit == 0).next().unwrap_or_default();
    String::from_utf16_lossy(text)
}

/// The strings in a buffer holding nul-terminated strings one after another, ending with an
/// empty string, as some Windows calls return. Stops at the end of the buffer if the empty
/// string is missing.
pub(crate) fn split_multi(buffer: &[u16]) -> impl Iterator<Item = &[u16]> {
    buffer
        .split(|&unit| unit == 0)
        .take_while(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_a_nul_terminated_buffer() {
        let wide = to_wide("C:\\données");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(from_wide(&wide), "C:\\données");
    }

    #[test]
    fn stops_at_the_first_nul() {
        let buffer: Vec<u16> = "ab\0cd".encode_utf16().collect();
        assert_eq!(from_wide(&buffer), "ab");
        assert_eq!(from_wide(&[]), "");
        let unterminated: Vec<u16> = "ab".encode_utf16().collect();
        assert_eq!(from_wide(&unterminated), "ab");
    }

    #[test]
    fn splits_a_list_of_strings() {
        let buffer: Vec<u16> = "C:\\\0D:\\mnt\\\0\0garbage".encode_utf16().collect();
        let names: Vec<String> = split_multi(&buffer).map(from_wide).collect();
        assert_eq!(names, ["C:\\", "D:\\mnt\\"]);
        assert_eq!(split_multi(&[0, 0]).count(), 0);
        let unterminated: Vec<u16> = "C:\\".encode_utf16().collect();
        assert_eq!(split_multi(&unterminated).count(), 1);
    }
}
