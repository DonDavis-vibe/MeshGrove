// Flat-shaded mesh. Normals come from screen-space derivatives, so the vertex
// buffer only holds positions and the (often wrong) STL normals are ignored.

struct Uniforms {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,          // xyz = camera position (world)
    base_color: vec4<f32>,   // linear RGB
    key_dir: vec4<f32>,      // xyz = direction *towards* the key light (world)
    fill_dir: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.world = pos;
    out.clip = u.view_proj * vec4<f32>(pos, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    let to_eye = normalize(u.eye.xyz - in.world);
    // Two-sided: always shade the side we look at (STL winding is unreliable, meshes may be open).
    if dot(n, to_eye) < 0.0 {
        n = -n;
    }

    let key = max(dot(n, u.key_dir.xyz), 0.0);
    let fill = max(dot(n, u.fill_dir.xyz), 0.0);
    // Hemisphere ambient: brighter from above (Z-up), darker from below.
    let hemi = mix(0.18, 0.38, n.z * 0.5 + 0.5);
    let rim = pow(1.0 - max(dot(n, to_eye), 0.0), 3.0) * 0.25;
    let half_v = normalize(u.key_dir.xyz + to_eye);
    let spec = pow(max(dot(n, half_v), 0.0), 48.0) * 0.18;

    let lit = u.base_color.rgb * (hemi + key * 0.85 + fill * 0.12) + vec3<f32>(spec + rim);
    return vec4<f32>(lit, 1.0);
}
