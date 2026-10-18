//! Capture variants: native vs. the DLSS quality modes, optionally combined with forward
//! rendering or TAA.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DlssMode {
    Dlaa,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}

impl DlssMode {
    pub const ALL: [DlssMode; 5] = [
        DlssMode::Dlaa,
        DlssMode::Quality,
        DlssMode::Balanced,
        DlssMode::Performance,
        DlssMode::UltraPerformance,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            DlssMode::Dlaa => "dlaa",
            DlssMode::Quality => "quality",
            DlssMode::Balanced => "balanced",
            DlssMode::Performance => "performance",
            DlssMode::UltraPerformance => "ultra-performance",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            DlssMode::Dlaa => "DLAA",
            DlssMode::Quality => "Quality",
            DlssMode::Balanced => "Balanced",
            DlssMode::Performance => "Performance",
            DlssMode::UltraPerformance => "Ultra Performance",
        }
    }

    /// Nominal per-axis render scale, only used to label a shot before the real
    /// `MainPassResolutionOverride` has been read back from the render world.
    pub fn nominal_scale(self) -> f32 {
        match self {
            DlssMode::Dlaa => 1.0,
            DlssMode::Quality => 2.0 / 3.0,
            DlssMode::Balanced => 0.58,
            DlssMode::Performance => 0.5,
            DlssMode::UltraPerformance => 1.0 / 3.0,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.slug() == s)
    }

    #[cfg(feature = "dlss")]
    pub fn to_bevy(self) -> bevy::anti_alias::dlss::DlssPerfQualityMode {
        use bevy::anti_alias::dlss::DlssPerfQualityMode as M;
        match self {
            DlssMode::Dlaa => M::Dlaa,
            DlssMode::Quality => M::Quality,
            DlssMode::Balanced => M::Balanced,
            DlssMode::Performance => M::Performance,
            DlssMode::UltraPerformance => M::UltraPerformance,
        }
    }
}

/// One capture of a scene. Every variant uses the same camera, the same prepasses
/// (depth + motion vectors, as DLSS requires) and HDR, so DLSS is the only difference
/// between `native` and `dlss-*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variant {
    pub dlss: Option<DlssMode>,
    /// Render opaque geometry with the forward path even if the scene defaults to deferred.
    pub forward: bool,
    pub taa: bool,
}

impl Variant {
    pub const NATIVE: Variant = Variant {
        dlss: None,
        forward: false,
        taa: false,
    };

    pub fn name(&self) -> String {
        let mut s = match self.dlss {
            Some(mode) => format!("dlss-{}", mode.slug()),
            None => "native".to_owned(),
        };
        if self.forward {
            s.push_str("-forward");
        }
        if self.taa {
            s.push_str("-taa");
        }
        s
    }

    pub fn parse(name: &str) -> Option<Self> {
        let mut rest = name;
        let mut v = Variant::NATIVE;
        if let Some(r) = rest.strip_suffix("-taa") {
            v.taa = true;
            rest = r;
        }
        if let Some(r) = rest.strip_suffix("-forward") {
            v.forward = true;
            rest = r;
        }
        if rest == "native" {
            return Some(v);
        }
        v.dlss = Some(DlssMode::parse(rest.strip_prefix("dlss-")?)?);
        Some(v)
    }

    /// Short human description used in labels, e.g. "DLSS Quality + TAA".
    pub fn describe(&self) -> String {
        let mut s = match self.dlss {
            Some(mode) => format!("DLSS {}", mode.display()),
            None => "DLSS off (native)".to_owned(),
        };
        if self.taa {
            s.push_str(" + TAA");
        }
        if self.forward {
            s.push_str(" + forced forward");
        }
        s
    }
}

impl fmt::Display for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for mode in DlssMode::ALL {
            for forward in [false, true] {
                for taa in [false, true] {
                    let v = Variant {
                        dlss: Some(mode),
                        forward,
                        taa,
                    };
                    assert_eq!(Variant::parse(&v.name()), Some(v));
                }
            }
        }
        assert_eq!(Variant::parse("native"), Some(Variant::NATIVE));
        assert_eq!(Variant::parse("dlss-bogus"), None);
    }
}
