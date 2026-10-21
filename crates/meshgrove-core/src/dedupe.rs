//! Duplicate detection: an exact content hash plus a geometry signature that
//! survives re-export (ASCII/binary STL, STL→3MF), moves, translation, rotation and mirroring.

use glam::{DMat3, DVec3};

use crate::mesh::Mesh;

/// Hex blake3 of the raw file bytes; equal hash = byte-identical copy.
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Rotation/translation-invariant geometry fingerprint.
///
/// Built from the triangle count, surface area, |enclosed volume| and the sorted
/// eigenvalues of the area-weighted surface covariance. All values are rounded to
/// 4 significant digits so float noise from re-exporting does not break equality.
/// (Axis-aligned box extents are deliberately left out: they change under arbitrary rotations.)
pub fn geo_signature(mesh: &Mesh) -> Option<String> {
    let bounds = mesh.bounds()?;
    // Work relative to the box center in f64 so large coordinates keep their precision.
    let origin = bounds.center().as_dvec3();
    let tri = |t: [glam::Vec3; 3]| t.map(|v| v.as_dvec3() - origin);

    let mut area = 0.0;
    let mut weighted_centroid = DVec3::ZERO;
    for [a, b, c] in mesh.triangles().map(tri) {
        let ta = 0.5 * (b - a).cross(c - a).length();
        if ta.is_finite() {
            area += ta;
            weighted_centroid += ta * (a + b + c) / 3.0;
        }
    }
    if area <= 0.0 {
        return None;
    }
    let centroid = weighted_centroid / area;

    let mut second_moment = DMat3::ZERO;
    let mut volume = 0.0;
    for t in mesh.triangles().map(tri) {
        let [a, b, c] = t.map(|v| v - centroid);
        let ta = 0.5 * (b - a).cross(c - a).length();
        if !ta.is_finite() {
            continue;
        }
        // ∫ x xᵀ dA over a triangle = A/12 · (Σ vᵢvᵢᵀ + s sᵀ), s = a + b + c
        let s = a + b + c;
        second_moment += (outer(a) + outer(b) + outer(c) + outer(s)) * (ta / 12.0);
        volume += a.dot(b.cross(c)) / 6.0;
    }
    let covariance = second_moment * (1.0 / area);
    let mut eig = symmetric_eigenvalues(covariance);
    let largest = eig[2].abs().max(f64::MIN_POSITIVE);
    for e in &mut eig {
        // Flat parts have a zero eigenvalue that otherwise shows up as random 1e-13 noise.
        if e.abs() < largest * 1e-6 {
            *e = 0.0;
        }
    }
    let volume = if volume.abs() < 1e-6 * area.powf(1.5) { 0.0 } else { volume.abs() };

    Some(format!("{}|{:.3e}|{:.3e}|{:.3e},{:.3e},{:.3e}", mesh.triangle_count(), area, volume, eig[0], eig[1], eig[2]))
}

fn outer(v: DVec3) -> DMat3 {
    DMat3::from_cols(v * v.x, v * v.y, v * v.z)
}

/// Eigenvalues of a symmetric 3×3 matrix, ascending (closed-form trigonometric method).
fn symmetric_eigenvalues(m: DMat3) -> [f64; 3] {
    let (a11, a22, a33) = (m.x_axis.x, m.y_axis.y, m.z_axis.z);
    let (a12, a13, a23) = (m.y_axis.x, m.z_axis.x, m.z_axis.y);
    let p1 = a12 * a12 + a13 * a13 + a23 * a23;
    let mut eig = if p1 == 0.0 {
        [a11, a22, a33]
    } else {
        let q = (a11 + a22 + a33) / 3.0;
        let p2 = (a11 - q).powi(2) + (a22 - q).powi(2) + (a33 - q).powi(2) + 2.0 * p1;
        let p = (p2 / 6.0).sqrt();
        let b = (m - DMat3::IDENTITY * q) * (1.0 / p);
        let r = (b.determinant() / 2.0).clamp(-1.0, 1.0);
        let phi = r.acos() / 3.0;
        let e1 = q + 2.0 * p * phi.cos();
        let e3 = q + 2.0 * p * (phi + 2.0 * std::f64::consts::PI / 3.0).cos();
        [e1, 3.0 * q - e1 - e3, e3]
    };
    eig.sort_by(f64::total_cmp);
    eig
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaders::stl;
    use glam::{Quat, Vec3};

    fn box_mesh(x: f32, y: f32, z: f32) -> Mesh {
        let mut m = Mesh::cube(1.0);
        for p in &mut m.positions {
            *p = [p[0] * x, p[1] * y, p[2] * z];
        }
        m
    }

    fn transformed(mesh: &Mesh, rot: Quat, offset: Vec3) -> Mesh {
        let mut m = mesh.clone();
        for p in &mut m.positions {
            *p = (rot * Vec3::from(*p) + offset).to_array();
        }
        m
    }

    #[test]
    fn identical_bytes_identical_hash() {
        let bytes = stl::write_binary(&box_mesh(20.0, 10.0, 5.0), "a");
        assert_eq!(content_hash(&bytes), content_hash(&bytes.clone()));
        assert_ne!(content_hash(&bytes), content_hash(&stl::write_binary(&box_mesh(20.0, 10.0, 5.0), "b")));
    }

    #[test]
    fn ascii_and_binary_export_match() {
        let m = box_mesh(20.0, 10.0, 5.0);
        let a = stl::parse(stl::write_ascii(&m, "x").as_bytes()).unwrap();
        let b = stl::parse(&stl::write_binary(&m, "x")).unwrap();
        assert_eq!(geo_signature(&a), geo_signature(&b));
        assert_eq!(geo_signature(&a), geo_signature(&m), "indexed vs soup must not matter");
    }

    #[test]
    fn moved_and_rotated_copies_match() {
        let m = box_mesh(20.0, 10.0, 5.0);
        let sig = geo_signature(&m).unwrap();
        let rot90 = transformed(&m, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), Vec3::new(100.0, -50.0, 3.0));
        let rot_odd =
            transformed(&m, Quat::from_axis_angle(Vec3::new(1.0, 2.0, 0.5).normalize(), 0.65), Vec3::splat(7.0));
        assert_eq!(geo_signature(&rot90).unwrap(), sig);
        assert_eq!(geo_signature(&rot_odd).unwrap(), sig);
    }

    #[test]
    fn mirrored_copy_matches() {
        let m = box_mesh(20.0, 10.0, 5.0);
        let mut mirrored = m.clone();
        for p in &mut mirrored.positions {
            p[0] = -p[0];
        }
        assert_eq!(geo_signature(&mirrored), geo_signature(&m));
    }

    #[test]
    fn slightly_scaled_or_different_shapes_do_not_match() {
        let m = box_mesh(20.0, 10.0, 5.0);
        assert_ne!(geo_signature(&box_mesh(20.2, 10.1, 5.05)), geo_signature(&m));
        assert_ne!(geo_signature(&box_mesh(20.0, 5.0, 10.0).clone()), geo_signature(&box_mesh(20.0, 20.0, 2.5)));
    }

    #[test]
    fn eigenvalues_of_known_matrix() {
        let m = DMat3::from_cols(DVec3::new(2.0, 1.0, 0.0), DVec3::new(1.0, 2.0, 0.0), DVec3::new(0.0, 0.0, 5.0));
        let e = symmetric_eigenvalues(m);
        for (got, want) in e.iter().zip([1.0, 3.0, 5.0]) {
            assert!((got - want).abs() < 1e-9, "{e:?}");
        }
    }
}
