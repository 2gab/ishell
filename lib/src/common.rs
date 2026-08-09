/// Constant strings for Unknown values
pub mod const_unknown {
    use crate::const_str;

    const_str! {
        UNKNOWN_ARTIST "Unknown Artist",
        UNKNOWN_TITLE "Unknown Title",
        UNKNOWN_ALBUM "Unknown Album",
        UNKNOWN_FILE "Unknown File",
    }
}

/// Small, dependency-free display-formatting helpers shared between the TUI and other
/// consumers (e.g. the Discord Rich Presence integration) that don't want to pull in a full
/// number-formatting crate just for this.
pub mod fmt {
    /// Format `n` with `,` as a thousands separator (`23154` -> `"23,154"`).
    #[must_use]
    pub fn group_thousands(n: u64) -> String {
        let digits = n.to_string();
        let mut out = String::with_capacity(digits.len() + digits.len() / 3);

        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(ch);
        }

        out
    }

    #[cfg(test)]
    mod tests {
        use super::group_thousands;

        #[test]
        fn group_thousands_formats_correctly() {
            assert_eq!(group_thousands(0), "0");
            assert_eq!(group_thousands(7), "7");
            assert_eq!(group_thousands(999), "999");
            assert_eq!(group_thousands(1000), "1,000");
            assert_eq!(group_thousands(23_154), "23,154");
            assert_eq!(group_thousands(1_234_567), "1,234,567");
        }
    }
}
