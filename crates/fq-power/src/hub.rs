//! Power trading hubs and zones.
//!
//! Each hub belongs to one ISO, which fixes its peak convention. Codes are
//! the common desk short names.

use fq_commod_core::{Commodity, Hub, Unit};

use crate::iso::Iso;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PowerHub {
    /// PJM Western Hub, the most liquid US power point.
    PjmWest,
    /// PJM AEP-Dayton Hub.
    PjmAdHub,
    /// PJM Northern Illinois Hub.
    PjmNiHub,
    /// PJM Eastern Hub.
    PjmEast,
    /// MISO Indiana Hub.
    MisoIndiana,
    /// MISO Minnesota Hub.
    MisoMinnesota,
    /// ERCOT North Hub.
    ErcotNorth,
    /// ERCOT Houston Hub.
    ErcotHouston,
    /// ERCOT West Hub.
    ErcotWest,
    /// ERCOT South Hub.
    ErcotSouth,
    /// NYISO Zone A (West).
    NyisoZoneA,
    /// NYISO Zone G (Hudson Valley).
    NyisoZoneG,
    /// NYISO Zone J (NYC).
    NyisoZoneJ,
    /// ISO-NE Mass Hub (Internal Hub).
    IsoneMassHub,
    /// SPP South Hub.
    SppSouth,
    /// SPP North Hub.
    SppNorth,
    /// CAISO SP15.
    CaisoSp15,
    /// CAISO NP15.
    CaisoNp15,
    /// Mid-Columbia (Pacific Northwest).
    MidC,
    /// Palo Verde (Desert Southwest).
    PaloVerde,
}

impl PowerHub {
    pub const ALL: [PowerHub; 20] = [
        Self::PjmWest,
        Self::PjmAdHub,
        Self::PjmNiHub,
        Self::PjmEast,
        Self::MisoIndiana,
        Self::MisoMinnesota,
        Self::ErcotNorth,
        Self::ErcotHouston,
        Self::ErcotWest,
        Self::ErcotSouth,
        Self::NyisoZoneA,
        Self::NyisoZoneG,
        Self::NyisoZoneJ,
        Self::IsoneMassHub,
        Self::SppSouth,
        Self::SppNorth,
        Self::CaisoSp15,
        Self::CaisoNp15,
        Self::MidC,
        Self::PaloVerde,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::PjmWest => "PJMW",
            Self::PjmAdHub => "ADHUB",
            Self::PjmNiHub => "NIHUB",
            Self::PjmEast => "PJME",
            Self::MisoIndiana => "INDY",
            Self::MisoMinnesota => "MINN",
            Self::ErcotNorth => "ERN",
            Self::ErcotHouston => "ERH",
            Self::ErcotWest => "ERW",
            Self::ErcotSouth => "ERS",
            Self::NyisoZoneA => "NYA",
            Self::NyisoZoneG => "NYG",
            Self::NyisoZoneJ => "NYJ",
            Self::IsoneMassHub => "MASS",
            Self::SppSouth => "SPPS",
            Self::SppNorth => "SPPN",
            Self::CaisoSp15 => "SP15",
            Self::CaisoNp15 => "NP15",
            Self::MidC => "MIDC",
            Self::PaloVerde => "PV",
        }
    }

    pub const fn iso(self) -> Iso {
        match self {
            Self::PjmWest | Self::PjmAdHub | Self::PjmNiHub | Self::PjmEast => Iso::Pjm,
            Self::MisoIndiana | Self::MisoMinnesota => Iso::Miso,
            Self::ErcotNorth | Self::ErcotHouston | Self::ErcotWest | Self::ErcotSouth => Iso::Ercot,
            Self::NyisoZoneA | Self::NyisoZoneG | Self::NyisoZoneJ => Iso::Nyiso,
            Self::IsoneMassHub => Iso::Isone,
            Self::SppSouth | Self::SppNorth => Iso::Spp,
            Self::CaisoSp15 | Self::CaisoNp15 => Iso::Caiso,
            Self::MidC | Self::PaloVerde => Iso::Wecc,
        }
    }

    /// The core `Hub`, priced in $/MWh. The block is carried separately
    /// (see [`crate::curve::PowerCurve`]) so a hub has one identity.
    pub fn hub(self) -> Hub {
        Hub::new(self.code(), Commodity::Power, Unit::MWh)
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.code().eq_ignore_ascii_case(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_iso_mapping() {
        for h in PowerHub::ALL {
            assert_eq!(PowerHub::from_code(h.code()), Some(h));
            assert_eq!(h.hub().unit(), Unit::MWh);
        }
        assert_eq!(PowerHub::PjmWest.iso(), Iso::Pjm);
        assert_eq!(PowerHub::CaisoSp15.iso().peak_days_per_week(), 6);
        assert_eq!(PowerHub::ErcotNorth.iso().peak_days_per_week(), 5);
        assert!(PowerHub::from_code("nope").is_none());
    }
}
