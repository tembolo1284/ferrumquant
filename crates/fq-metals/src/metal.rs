//! Metal identifiers, units and contract sizes.

use fq_commod_core::{Commodity, Hub, Unit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Metal {
    /// XAU, USD per troy ounce, loco London.
    Gold,
    /// XAG, USD per troy ounce.
    Silver,
    /// XPT, USD per troy ounce.
    Platinum,
    /// XPD, USD per troy ounce.
    Palladium,
    /// LME Grade A copper, USD per tonne, 25 t lots.
    Copper,
    /// LME primary aluminium, USD per tonne, 25 t lots.
    Aluminium,
    /// LME special high grade zinc, USD per tonne, 25 t lots.
    Zinc,
    /// LME primary nickel, USD per tonne, 6 t lots.
    Nickel,
    /// LME standard lead, USD per tonne, 25 t lots.
    Lead,
    /// LME tin, USD per tonne, 5 t lots.
    Tin,
}

impl Metal {
    pub const PRECIOUS: [Metal; 4] = [Self::Gold, Self::Silver, Self::Platinum, Self::Palladium];
    pub const BASE: [Metal; 6] = [Self::Copper, Self::Aluminium, Self::Zinc, Self::Nickel, Self::Lead, Self::Tin];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Gold => "XAU",
            Self::Silver => "XAG",
            Self::Platinum => "XPT",
            Self::Palladium => "XPD",
            Self::Copper => "CA",
            Self::Aluminium => "AH",
            Self::Zinc => "ZS",
            Self::Nickel => "NI",
            Self::Lead => "PB",
            Self::Tin => "SN",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Gold => "Gold",
            Self::Silver => "Silver",
            Self::Platinum => "Platinum",
            Self::Palladium => "Palladium",
            Self::Copper => "Copper",
            Self::Aluminium => "Aluminium",
            Self::Zinc => "Zinc",
            Self::Nickel => "Nickel",
            Self::Lead => "Lead",
            Self::Tin => "Tin",
        }
    }

    pub const fn is_precious(self) -> bool {
        matches!(self, Self::Gold | Self::Silver | Self::Platinum | Self::Palladium)
    }

    pub const fn commodity(self) -> Commodity {
        if self.is_precious() { Commodity::PreciousMetal } else { Commodity::BaseMetal }
    }

    pub const fn unit(self) -> Unit {
        if self.is_precious() { Unit::TroyOunce } else { Unit::MetricTon }
    }

    /// LME lot size in tonnes; precious metals trade OTC in ounces with no
    /// standard lot, reported here as 1.
    pub const fn lot_size(self) -> f64 {
        match self {
            Self::Copper | Self::Aluminium | Self::Zinc | Self::Lead => 25.0,
            Self::Nickel => 6.0,
            Self::Tin => 5.0,
            _ => 1.0,
        }
    }

    /// Furthest LME prompt listed, in months from the trade date.
    pub const fn lme_horizon_months(self) -> i32 {
        match self {
            Self::Tin => 15,
            Self::Copper | Self::Aluminium | Self::Zinc | Self::Nickel | Self::Lead => 123,
            _ => 0,
        }
    }

    pub fn hub(self) -> Hub {
        Hub::new(self.code(), self.commodity(), self.unit())
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::PRECIOUS.into_iter().chain(Self::BASE).find(|m| m.code().eq_ignore_ascii_case(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_classification() {
        for m in Metal::PRECIOUS.into_iter().chain(Metal::BASE) {
            assert_eq!(Metal::from_code(m.code()), Some(m));
        }
        assert!(Metal::Gold.is_precious() && !Metal::Copper.is_precious());
        assert_eq!(Metal::Gold.unit(), Unit::TroyOunce);
        assert_eq!(Metal::Nickel.unit(), Unit::MetricTon);
        assert!((Metal::Nickel.lot_size() - 6.0).abs() < 1e-12);
        assert_eq!(Metal::Tin.lme_horizon_months(), 15);
        assert_eq!(Metal::Copper.lme_horizon_months(), 123);
        assert!(Metal::from_code("xyz").is_none());
    }
}
