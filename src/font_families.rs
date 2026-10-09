//! Pure family-name selection for the embedded and system fonts.

pub const GEIST: &str = "Geist";
pub const GEIST_MONO: &str = "Geist Mono";
pub const SYSTEM_FALLBACK: &str = "Segoe UI";

pub const BUNDLED_FAMILY_NAMES: [&str; 4] = [
    GEIST,
    GEIST_MONO,
    "Plus Jakarta Sans",
    "Noto Sans Devanagari",
];

pub fn is_bundled_family(family: &str) -> bool {
    BUNDLED_FAMILY_NAMES
        .iter()
        .any(|name| name.eq_ignore_ascii_case(family.trim()))
}

pub fn family_for_collection(family: &str, collection_available: bool) -> &str {
    if is_bundled_family(family) && !collection_available {
        SYSTEM_FALLBACK
    } else {
        family
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_families_resolve_case_insensitively_and_fall_back_safely() {
        assert!(is_bundled_family("Geist"));
        assert!(is_bundled_family("GEIST MONO"));
        assert!(is_bundled_family("Plus Jakarta Sans"));
        assert!(!is_bundled_family("Arial"));
        assert_eq!(family_for_collection("Geist", false), SYSTEM_FALLBACK);
        assert_eq!(family_for_collection("Arial", false), "Arial");
        assert_eq!(family_for_collection("Geist", true), "Geist");
    }
}
