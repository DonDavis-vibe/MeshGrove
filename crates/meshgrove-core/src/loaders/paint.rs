//! Multi-material painting of slicer 3MF projects: the `paint_color` (Bambu Studio, OrcaSlicer,
//! Anycubic) and `slic3rpe:mmu_segmentation` (PrusaSlicer) triangle attributes.
//!
//! Each painted triangle carries a hex string that encodes a subdivision tree: a triangle is
//! either a leaf with a state (0 = the part's own filament, k = filament k) or split along one,
//! two or three of its sides into 2–4 children. This mirrors PrusaSlicer's
//! `TriangleSelector::deserialize` / `perform_split`, which the other slicers share.

use glam::Vec3;

/// A leaf of the subdivision: its corners and its paint state.
pub type Leaf = ([Vec3; 3], u8);

/// Splits `corners` as encoded by `code`. Returns `None` for malformed data, in which case the
/// slicers drop the paint of the triangle as well.
pub fn decode(code: &str, corners: [Vec3; 3]) -> Option<Vec<Leaf>> {
    let mut bits = Bits::new(code)?;
    let mut leaves = Vec::new();
    node(&mut bits, corners, &mut leaves, 0)?;
    Some(leaves)
}

/// The bit stream: the hex string read from its end, each digit least significant bit first.
struct Bits {
    bits: Vec<bool>,
    next: usize,
}

impl Bits {
    fn new(code: &str) -> Option<Bits> {
        let mut bits = Vec::with_capacity(code.len() * 4);
        for c in code.trim().chars().rev() {
            let digit = c.to_digit(16)?;
            bits.extend((0..4).map(|i| digit & (1 << i) != 0));
        }
        Some(Bits { bits, next: 0 })
    }

    fn nibble(&mut self) -> Option<u32> {
        let slice = self.bits.get(self.next..self.next + 4)?;
        self.next += 4;
        Some(slice.iter().enumerate().map(|(i, &b)| u32::from(b) << i).sum())
    }
}

/// Deeper trees than this do not come out of a slicer; they guard against garbage input.
const MAX_DEPTH: usize = 24;

fn node(bits: &mut Bits, corners: [Vec3; 3], leaves: &mut Vec<Leaf>, depth: usize) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    let code = bits.nibble()?;
    let split_sides = (code & 0b11) as usize;
    if split_sides == 0 {
        let state = match code >> 2 {
            // "11" announces a bigger state in the next nibble (0b1111 → yet another one).
            0b11 => match bits.nibble()? {
                0b1111 => bits.nibble()? + 18,
                n => n + 3,
            },
            s => s,
        };
        leaves.push((corners, u8::try_from(state).ok()?));
        return Some(());
    }
    let special = if split_sides == 3 { 0 } else { (code >> 2) as usize };
    if special > 2 {
        return None;
    }
    let children = split(corners, split_sides, special);
    // Children are stored last to first.
    for child in children.into_iter().rev() {
        node(bits, child, leaves, depth + 1)?;
    }
    Some(())
}

/// The children of a split triangle in `perform_split` order. The corners start at the special
/// side: the split side for one split, the kept side for two.
fn split(corners: [Vec3; 3], sides: usize, special: usize) -> Vec<[Vec3; 3]> {
    let v = [corners[special], corners[(special + 1) % 3], corners[(special + 2) % 3]];
    let mid = |a: Vec3, b: Vec3| (a + b) * 0.5;
    match sides {
        1 => {
            let m = mid(v[2], v[1]);
            vec![[v[0], v[1], m], [m, v[2], v[0]]]
        }
        2 => {
            let (m01, m02) = (mid(v[1], v[0]), mid(v[0], v[2]));
            vec![[v[0], m01, m02], [m01, v[1], m02], [v[1], v[2], m02]]
        }
        _ => {
            let (m01, m12, m02) = (mid(v[1], v[0]), mid(v[2], v[1]), mid(v[0], v[2]));
            vec![[v[0], m01, m02], [m01, v[1], m12], [m12, v[2], m02], [m01, m12, m02]]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri() -> [Vec3; 3] {
        [Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0), Vec3::new(0.0, 4.0, 0.0)]
    }

    fn area(t: &[Vec3; 3]) -> f32 {
        (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5
    }

    #[test]
    fn whole_triangle_states() {
        // A leaf nibble holds the state in its upper two bits.
        assert_eq!(decode("4", tri()).unwrap(), vec![(tri(), 1)]);
        assert_eq!(decode("8", tri()).unwrap(), vec![(tri(), 2)]);
        // "11" + next nibble (+3): state 3 + 4 = 7. The string is read from its end.
        assert_eq!(decode("4C", tri()).unwrap(), vec![(tri(), 7)]);
        // 0b1111 escape: state 18 + 2 = 20.
        assert_eq!(decode("2FC", tri()).unwrap(), vec![(tri(), 20)]);
    }

    #[test]
    fn three_way_split_covers_the_triangle() {
        // Split all three sides (nibble 3), then four leaves stored last child first:
        // states 1, 2, 1, 2 → children 3, 2, 1, 0 get them in that order.
        let leaves = decode("84843", tri()).unwrap();
        assert_eq!(leaves.len(), 4);
        let total: f32 = leaves.iter().map(|(t, _)| area(t)).sum();
        assert!((total - area(&tri())).abs() < 1e-5);
        // The middle triangle (child 3) is read first and got state 1.
        let middle = leaves.iter().find(|(t, _)| t.contains(&Vec3::new(2.0, 2.0, 0.0)) && !t.contains(&Vec3::ZERO));
        assert!(middle.is_some());
        assert_eq!(leaves[0].1, 1);
        assert!(leaves[0].0.contains(&Vec3::new(2.0, 0.0, 0.0)) && leaves[0].0.contains(&Vec3::new(0.0, 2.0, 0.0)));
    }

    #[test]
    fn one_and_two_side_splits_keep_the_area() {
        for (code, n) in [("481", 2), ("445", 2), ("84846", 3)] {
            let leaves = decode(code, tri()).unwrap();
            assert_eq!(leaves.len(), n, "{code}");
            let total: f32 = leaves.iter().map(|(t, _)| area(t)).sum();
            assert!((total - area(&tri())).abs() < 1e-5, "{code}");
        }
    }

    #[test]
    fn real_slicer_string_decodes() {
        // From a Bambu/Orca-family project in the wild: nested splits, two filaments.
        let code = "000C03000C030C00C0C0C33000C03000C030C00C0C0C330C00C0C0C30C0C0C33";
        let leaves = decode(code, tri()).unwrap();
        let total: f32 = leaves.iter().map(|(t, _)| area(t)).sum();
        assert!((total - area(&tri())).abs() < 1e-4);
        assert!(leaves.len() > 8);
        assert!(leaves.iter().all(|(_, s)| *s <= 4));
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(decode("xyz", tri()).is_none());
        assert!(decode("3", tri()).is_none(), "split without children");
        assert!(decode("", tri()).is_none());
    }
}
