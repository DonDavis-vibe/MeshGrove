// Flat-shaded mesh. Normals come from screen-space derivatives, so the vertex
// buffer only holds positions and the (often wrong) STL normals are ignored.
// Coloured meshes (3MF filaments, materials) add an sRGB colour per vertex.

struct Uniforms {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,          // xyz = camera position (world), w = 2 for studio lighting
    base_color: vec4<f32>,   // linear RGB
    key_dir: vec4<f32>,      // xyz = direction *towards* the key light (world)
    fill_dir: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) color: vec3<f32>,   // linear RGB, the same at all corners of a triangle
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.world = pos;
    out.clip = u.view_proj * vec4<f32>(pos, 1.0);
    out.color = u.base_color.rgb;
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_colored(@location(0) pos: vec3<f32>, @location(1) color: vec4<f32>) -> VsOut {
    var out: VsOut;
    out.world = pos;
    out.clip = u.view_proj * vec4<f32>(pos, 1.0);
    out.color = srgb_to_linear(color.rgb);
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

    var lit = in.color * (hemi + key * 0.85 + fill * 0.12) + vec3<f32>(spec + rim);
    if u.eye.w > 1.5 {
        // Studio: a kicker from behind, a tight glossy highlight and a stronger edge glow.
        let kicker = max(dot(n, normalize(vec3<f32>(-u.key_dir.xy, 0.6))), 0.0);
        let gloss = pow(max(dot(n, half_v), 0.0), 160.0) * 0.45;
        let edge = pow(1.0 - max(dot(n, to_eye), 0.0), 4.0) * 0.45;
        lit = in.color * (hemi * 1.1 + key * 0.9 + fill * 0.2 + kicker * 0.18) + vec3<f32>(spec * 0.8 + gloss + edge);
    }
    return vec4<f32>(lit, 1.0);
}
