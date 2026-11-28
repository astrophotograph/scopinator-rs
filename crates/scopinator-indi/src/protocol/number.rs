//! Numbers as INDI writes them.
//!
//! A number travels as text. Besides the usual decimal form a server may
//! write it in sexagesimal (`12:30:45.5`), which mounts use for right
//! ascension and declination.

/// Read a number: a decimal (`1.5`, `-2`, `1e3`), or one to three
/// sexagesimal fields (`d`, `d:m`, `d:m:s`) separated by colons, spaces or
/// semicolons. A leading minus sign applies to the whole value, so
/// `-00:30:00` is -0.5. `None` for anything else.
pub fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(value) = text.parse::<f64>() {
        // `parse` also takes "NaN" and "inf", which are not numbers here.
        return value.is_finite().then_some(value);
    }

    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let fields: Vec<&str> = body
        .split([':', ' ', ';'])
        .filter(|field| !field.is_empty())
        .collect();
    if !(2..=3).contains(&fields.len()) {
        return None;
    }
    let mut value = 0.0;
    for (field, divisor) in fields.iter().zip([1.0, 60.0, 3600.0]) {
        // Only the whole value has a sign, and no field is an exponent.
        if !field.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        value += field.parse::<f64>().ok()? / divisor;
    }
    let value = if negative { -value } else { value };
    value.is_finite().then_some(value)
}

/// A number as a client sends it: the shortest decimal that reads back as
/// the same value. Never sexagesimal.
pub fn format_number(value: f64) -> String {
    debug_assert!(value.is_finite(), "{value} is not a number INDI can carry");
    if value.is_finite() {
        value.to_string()
    } else {
        "0".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_are_read() {
        assert_eq!(parse_number("1.5"), Some(1.5));
        assert_eq!(parse_number("-2"), Some(-2.0));
        assert_eq!(parse_number(" 3.25 "), Some(3.25));
        assert_eq!(parse_number("1e3"), Some(1000.0));
        assert_eq!(parse_number("+7"), Some(7.0));
    }

    #[test]
    fn sexagesimal_is_read() {
        assert_eq!(parse_number("12:30:45"), Some(12.5125));
        assert_eq!(parse_number("12 30 45"), Some(12.5125));
        assert_eq!(parse_number("12;30;45"), Some(12.5125));
        assert_eq!(parse_number("-05:30"), Some(-5.5));
        assert_eq!(
            parse_number("12:30:45.5"),
            Some(12.0 + 30.0 / 60.0 + 45.5 / 3600.0)
        );
        // Runs of separators, as "%10.6m" pads its fields.
        assert_eq!(parse_number(" 5: 7: 30"), Some(5.125));
    }

    #[test]
    fn the_sign_belongs_to_the_whole_value() {
        // Minus zero degrees is still south of the equator.
        assert_eq!(parse_number("-00:30:00"), Some(-0.5));
        assert_eq!(parse_number("-0:0:36"), Some(-0.01));
    }

    #[test]
    fn what_is_not_a_number_is_none() {
        for text in [
            "", "   ", "abc", "1:2:3:4", "NaN", "inf", "-inf", "12:xx", "1:-2", "1:2e3", ":",
        ] {
            assert_eq!(parse_number(text), None, "{text:?}");
        }
    }

    #[test]
    fn numbers_are_written_as_plain_decimals() {
        assert_eq!(format_number(12.5), "12.5");
        assert_eq!(format_number(45.0), "45");
        assert_eq!(format_number(-0.25), "-0.25");
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn a_written_number_reads_back(value in -1.0e12f64..1.0e12) {
                prop_assert_eq!(parse_number(&format_number(value)), Some(value));
            }

            #[test]
            fn parsing_never_panics(text in ".{0,40}") {
                let _ = parse_number(&text);
            }
        }
    }
}
