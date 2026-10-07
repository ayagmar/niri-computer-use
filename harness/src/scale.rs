use std::fmt;
use std::str::FromStr;

use crate::failure::Failure;

/// The nested output scale written to the niri config.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scale(f64);

impl Scale {
    pub(crate) const ONE: Self = Self(1.0);

    /// niri reports the scale it was configured with, so only float noise is tolerated.
    pub(crate) fn matches(self, reported: f64) -> bool {
        (self.0 - reported).abs() < 1e-9
    }
}

impl FromStr for Scale {
    type Err = Failure;

    fn from_str(text: &str) -> Result<Self, Failure> {
        match text.parse::<f64>() {
            Ok(value) if value.is_finite() && value > 0.0 => Ok(Self(value)),
            _ => Err(Failure::new(format!("invalid scale {text:?}"))),
        }
    }
}

impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_plain_numbers() {
        assert_eq!("1".parse::<Scale>().unwrap().to_string(), "1");
        assert_eq!("1.5".parse::<Scale>().unwrap().to_string(), "1.5");
    }

    #[test]
    fn rejects_non_positive_and_non_numbers() {
        for text in ["0", "-1", "NaN", "inf", "x", ""] {
            assert!(text.parse::<Scale>().is_err(), "{text}");
        }
    }

    #[test]
    fn matches_the_reported_scale() {
        let scale: Scale = "1.5".parse().unwrap();
        assert!(scale.matches(1.5));
        assert!(!scale.matches(1.25));
    }
}
