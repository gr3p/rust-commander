/// How the visualizer draws the music.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VisualStyle {
    /// Spectrum bars with caps that fall back with gravity.
    #[default]
    Bars,
    /// The live wave as a glowing line.
    Wave,
    /// A ring of spectrum bars that pulses with the bass.
    Radial,
    /// The spectrum as one smooth filled curve.
    Ribbon,
}

impl VisualStyle {
    pub const ALL: [VisualStyle; 4] = [Self::Bars, Self::Wave, Self::Radial, Self::Ribbon];

    /// The name saved in config.toml.
    pub fn name(self) -> &'static str {
        match self {
            Self::Bars => "bars",
            Self::Wave => "wave",
            Self::Radial => "radial",
            Self::Ribbon => "ribbon",
        }
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bars => "Bars",
            Self::Wave => "Wave",
            Self::Radial => "Radial",
            Self::Ribbon => "Ribbon",
        }
    }

    /// Reads a saved name. Anything unknown means the default.
    pub fn from_name(name: &str) -> Self {
        let name = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|style| style.name() == name).unwrap_or_default()
    }

    /// The next style, for a button that steps through them.
    pub fn cycle(self) -> Self {
        let at = Self::ALL.iter().position(|style| *style == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_unknown_ones_fall_back() {
        for style in VisualStyle::ALL {
            assert_eq!(VisualStyle::from_name(style.name()), style);
            assert!(!style.label().is_empty());
        }
        assert_eq!(VisualStyle::from_name(" WAVE "), VisualStyle::Wave);
        assert_eq!(VisualStyle::from_name("lava lamp"), VisualStyle::Bars);
        assert_eq!(VisualStyle::from_name(""), VisualStyle::Bars);
    }

    #[test]
    fn cycling_visits_every_style_and_comes_back() {
        let mut style = VisualStyle::Bars;
        let mut seen = Vec::new();
        for _ in 0..VisualStyle::ALL.len() {
            seen.push(style);
            style = style.cycle();
        }
        assert_eq!(style, VisualStyle::Bars);
        assert_eq!(seen, VisualStyle::ALL.to_vec());
    }
}
