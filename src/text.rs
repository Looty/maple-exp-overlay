//! Small text formatting helpers shared by the overlay and the settings window.

/// 24855 -> "24,855"
pub fn commas(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::commas;

    #[test]
    fn groups_thousands() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(5140), "5,140");
        assert_eq!(commas(24_855), "24,855");
        assert_eq!(commas(9_482_161_548), "9,482,161,548");
    }
}
