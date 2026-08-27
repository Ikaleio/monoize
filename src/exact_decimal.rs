use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Multiplier(Decimal);

impl Multiplier {
    pub const ONE: Self = Self(Decimal::ONE);
    pub const ZERO: Self = Self(Decimal::ZERO);

    /// MP-C11/MP-C12 (`model-pricing.spec.md`): multipliers and billing
    /// ratios are non-negative base-10 decimal strings; `0` is a valid
    /// explicit zero-charge configuration.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty()
            || raw.starts_with('+')
            || raw.contains('e')
            || raw.contains('E')
            || !raw
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
            || raw.bytes().filter(|byte| *byte == b'.').count() > 1
        {
            return Err("multiplier must be a non-negative base-10 decimal string".to_string());
        }
        let value = Decimal::from_str(raw)
            .map_err(|_| "multiplier must be a non-negative base-10 decimal string".to_string())?;
        if value < Decimal::ZERO {
            return Err("multiplier must be >= 0".to_string());
        }
        if value.scale() > 9 {
            return Err("multiplier must have at most 9 fractional digits".to_string());
        }
        Ok(Self(value.normalize()))
    }

    pub fn is_positive(self) -> bool {
        self.0 > Decimal::ZERO
    }

    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    /// Exact product of two multipliers (MP-C11: `channel_multiplier ×
    /// group_billing_ratio` composes without intermediate rounding).
    pub fn compose(self, other: Self) -> Option<Self> {
        self.0
            .checked_mul(other.0)
            .map(|value| Self(value.normalize()))
    }

    pub fn checked_scale_i128(self, base: i128) -> Option<i128> {
        let coefficient = self.0.mantissa();
        let divisor = 10_i128.checked_pow(self.0.scale())?;
        let whole = base.checked_div(divisor)?;
        let remainder = base.checked_rem(divisor)?;
        whole
            .checked_mul(coefficient)?
            .checked_add(remainder.checked_mul(coefficient)?.checked_div(divisor)?)
    }

    pub fn canonical(self) -> String {
        self.0.normalize().to_string()
    }
}

impl Default for Multiplier {
    fn default() -> Self {
        Self::ONE
    }
}

impl Display for Multiplier {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.canonical())
    }
}

impl FromStr for Multiplier {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl Serialize for Multiplier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.canonical())
    }
}

impl<'de> Deserialize<'de> for Multiplier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::Multiplier;

    #[test]
    fn decimal_scaling_is_exact_and_truncates_toward_zero() {
        let multiplier = Multiplier::parse("1.001").unwrap();
        assert_eq!(multiplier.checked_scale_i128(1000), Some(1001));
        assert_eq!(multiplier.checked_scale_i128(1), Some(1));
        assert_eq!(multiplier.canonical(), "1.001");
    }

    #[test]
    fn invalid_or_negative_multiplier_is_rejected() {
        for raw in ["", "-1", "+1", "1e2", "NaN", "1.0000000001"] {
            assert!(Multiplier::parse(raw).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn zero_multiplier_is_accepted_and_scales_to_zero() {
        let zero = Multiplier::parse("0").unwrap();
        assert!(zero.is_zero());
        assert_eq!(zero.checked_scale_i128(123_456), Some(0));
        assert_eq!(zero.canonical(), "0");
    }

    #[test]
    fn compose_multiplies_exactly() {
        let a = Multiplier::parse("1.5").unwrap();
        let b = Multiplier::parse("0.3").unwrap();
        let product = a.compose(b).unwrap();
        assert_eq!(product.canonical(), "0.45");
        assert_eq!(product.checked_scale_i128(1000), Some(450));
        // trunc(999 * 0.45) = trunc(449.55) = 449
        assert_eq!(product.checked_scale_i128(999), Some(449));
    }
}
