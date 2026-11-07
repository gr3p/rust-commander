use symphonia::core::audio::Channels;

/// What a channel in a decoded frame carries, which decides how it folds into stereo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Left,
    Right,
    Centre,
    Lfe,
    SideLeft,
    SideRight,
    RearLeft,
    RearRight,
    RearCentre,
    Skip,
}

/// Roles in the order the channels sit in an interleaved frame (ascending channel bit).
pub fn roles_from_channels(channels: Channels) -> Vec<Role> {
    let mut roles = Vec::new();
    for bit in 0..32 {
        let flag = Channels::from_bits_truncate(1 << bit);
        if flag.is_empty() || !channels.contains(flag) {
            continue;
        }
        roles.push(role_of(flag));
    }
    roles
}

fn role_of(flag: Channels) -> Role {
    if flag == Channels::FRONT_LEFT || flag == Channels::FRONT_LEFT_CENTRE || flag == Channels::FRONT_LEFT_WIDE {
        Role::Left
    } else if flag == Channels::FRONT_RIGHT || flag == Channels::FRONT_RIGHT_CENTRE || flag == Channels::FRONT_RIGHT_WIDE {
        Role::Right
    } else if flag == Channels::FRONT_CENTRE {
        Role::Centre
    } else if flag == Channels::LFE1 || flag == Channels::LFE2 {
        Role::Lfe
    } else if flag == Channels::SIDE_LEFT {
        Role::SideLeft
    } else if flag == Channels::SIDE_RIGHT {
        Role::SideRight
    } else if flag == Channels::REAR_LEFT || flag == Channels::REAR_LEFT_CENTRE {
        Role::RearLeft
    } else if flag == Channels::REAR_RIGHT || flag == Channels::REAR_RIGHT_CENTRE {
        Role::RearRight
    } else if flag == Channels::REAR_CENTRE {
        Role::RearCentre
    } else {
        Role::Skip
    }
}

/// Vorbis channel order, which Opus uses for mapping family 1.
pub fn vorbis_roles(count: usize) -> Option<Vec<Role>> {
    use Role::*;
    Some(match count {
        1 => vec![Left],
        2 => vec![Left, Right],
        3 => vec![Left, Centre, Right],
        4 => vec![Left, Right, RearLeft, RearRight],
        5 => vec![Left, Centre, Right, RearLeft, RearRight],
        6 => vec![Left, Centre, Right, RearLeft, RearRight, Lfe],
        7 => vec![Left, Centre, Right, SideLeft, SideRight, RearCentre, Lfe],
        8 => vec![Left, Centre, Right, SideLeft, SideRight, RearLeft, RearRight, Lfe],
        _ => return None,
    })
}

const MIX: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Folds any layout into mono or stereo. One and two channels pass through unchanged.
pub struct Downmix {
    roles: Vec<Role>,
    // (left gain, right gain) per input channel.
    gains: Vec<(f32, f32)>,
}

impl Downmix {
    pub fn new(roles: Vec<Role>) -> Self {
        let gains: Vec<(f32, f32)> = roles
            .iter()
            .map(|role| match role {
                Role::Left => (1.0, 0.0),
                Role::Right => (0.0, 1.0),
                Role::Centre => (MIX, MIX),
                Role::SideLeft | Role::RearLeft => (MIX, 0.0),
                Role::SideRight | Role::RearRight => (0.0, MIX),
                Role::RearCentre => (0.5, 0.5),
                Role::Lfe | Role::Skip => (0.0, 0.0),
            })
            .collect();
        Self { roles, gains }
    }

    pub fn inputs(&self) -> usize {
        self.roles.len()
    }

    /// 1 for a mono source, otherwise 2.
    pub fn outputs(&self) -> usize {
        if self.roles.len() == 1 { 1 } else { 2 }
    }

    pub fn passthrough(&self) -> bool {
        self.roles.len() <= 2
    }

    /// Appends the mixed frames to `out`. A trailing partial frame is dropped.
    pub fn apply(&self, input: &[f32], out: &mut Vec<f32>) {
        let n = self.roles.len();
        if n == 0 {
            return;
        }
        if self.passthrough() {
            out.extend_from_slice(&input[..input.len() / n * n]);
            return;
        }
        // Power-preserving scale: a loud surround mix stays near the level of a stereo one.
        let (mut left_power, mut right_power) = (0.0f32, 0.0f32);
        for (l, r) in &self.gains {
            left_power += l * l;
            right_power += r * r;
        }
        let left_scale = 1.0 / left_power.sqrt().max(1.0);
        let right_scale = 1.0 / right_power.sqrt().max(1.0);
        out.reserve(input.len() / n * 2);
        for frame in input.chunks_exact(n) {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for (sample, (gl, gr)) in frame.iter().zip(&self.gains) {
                l += sample * gl;
                r += sample * gr;
            }
            out.push(l * left_scale);
            out.push(r * right_scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_and_mono_pass_through() {
        let stereo = Downmix::new(vec![Role::Left, Role::Right]);
        let mut out = Vec::new();
        stereo.apply(&[0.1, 0.2, 0.3, 0.4, 0.5], &mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(stereo.outputs(), 2);
        let mono = Downmix::new(vec![Role::Left]);
        assert_eq!(mono.outputs(), 1);
        assert!(mono.passthrough());
    }

    #[test]
    fn five_one_folds_to_stereo_without_clipping_a_full_scale_mix() {
        let roles = vorbis_roles(6).unwrap();
        let mix = Downmix::new(roles);
        // Every channel at full scale: the worst case for clipping.
        let mut out = Vec::new();
        mix.apply(&[1.0; 6], &mut out);
        assert_eq!(out.len(), 2);
        assert!(out[0] <= 2.0 && out[1] <= 2.0, "{out:?}");
        // Left only reaches the left side.
        let mut left = Vec::new();
        mix.apply(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0], &mut left);
        assert!(left[0] > 0.5 && left[1] == 0.0, "{left:?}");
        // The LFE channel is dropped.
        let mut lfe = Vec::new();
        mix.apply(&[0.0, 0.0, 0.0, 0.0, 0.0, 1.0], &mut lfe);
        assert_eq!(lfe, vec![0.0, 0.0]);
        // The centre feeds both sides equally.
        let mut centre = Vec::new();
        mix.apply(&[0.0, 1.0, 0.0, 0.0, 0.0, 0.0], &mut centre);
        assert!(centre[0] > 0.0 && (centre[0] - centre[1]).abs() < 1e-6, "{centre:?}");
    }

    #[test]
    fn roles_follow_bit_order() {
        let ch = Channels::FRONT_LEFT | Channels::FRONT_RIGHT | Channels::FRONT_CENTRE | Channels::LFE1 | Channels::REAR_LEFT | Channels::REAR_RIGHT;
        assert_eq!(
            roles_from_channels(ch),
            vec![Role::Left, Role::Right, Role::Centre, Role::Lfe, Role::RearLeft, Role::RearRight]
        );
    }

    #[test]
    fn vorbis_layouts_cover_one_to_eight_channels() {
        for n in 1..=8 {
            assert_eq!(vorbis_roles(n).unwrap().len(), n);
        }
        assert!(vorbis_roles(9).is_none());
        assert!(vorbis_roles(0).is_none());
    }
}
