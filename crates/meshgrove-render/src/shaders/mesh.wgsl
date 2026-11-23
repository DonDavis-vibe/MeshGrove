// Flat-shaded mesh. Normals come from screen-space derivatives, so the vertex
// buffer only holds positions and the (often wrong) STL normals are ignored.
// Coloured meshes (3MF filaments, materials) add an sRGB colour per vertex.
// Wireframe: every triangle has three vertices of its own, so the number of a vertex
// says which corner it is, and that is all it takes to find the triangle's edges.

struct Uniforms {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,          // xyz = camera position (world), w = 2 for studio lighting
    base_color: vec4<f32>,   // linear RGB
    key_dir: vec4<f32>,      // xyz = direction *towards* the key light (world)
    fill_dir: vec4<f32>,
    wire: vec4<f32>,         // x = width of the triangles' edges in pixels, 0 for none
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) color: vec3<f32>,   // linear RGB, the same at all corners of a triangle
    @location(2) corner: vec3<f32>,  // 1 at one corner each: falls to 0 on the edge opposite
};

fn corner_of(index: u32) -> vec3<f32> {
    let c = index % 3u;
    return vec3<f32>(f32(c == 0u), f32(c == 1u), f32(c == 2u));
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32, @location(0) pos: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.world = pos;
    out.clip = u.view_proj * vec4<f32>(pos, 1.0);
    out.color = u.base_color.rgb;
    out.corner = corner_of(index);
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_colored(
    @builtin(vertex_index) index: u32,
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.world = pos;
    out.clip = u.view_proj * vec4<f32>(pos, 1.0);
    out.color = srgb_to_linear(color.rgb);
    out.corner = corner_of(index);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    // How fast each corner's share changes per pixel: 1 / the triangle's height there.
    let slope = fwidth(in.corner);
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
    if u.wire.x > 0.0 {
        // Pixels to each edge, and the triangle's height over it. Each of the two triangles at
        // an edge draws half the line.
        let per_pixel = max(slope, vec3<f32>(1e-6));
        let away = in.corner / per_pixel;
        let half = u.wire.x * 0.5;
        let drawn = vec3<f32>(1.0) - smoothstep(vec3<f32>(half - 0.5), vec3<f32>(half + 0.5), away);
        // Edges a few pixels apart would fill the surface: they fade, so that a fine mesh turns
        // a shade darker instead of solid.
        let fade = clamp(vec3<f32>(1.0) / (per_pixel * 6.0 * u.wire.x), vec3<f32>(0.2), vec3<f32>(1.0));
        let lines = drawn * fade;
        let line = max(lines.x, max(lines.y, lines.z));
        // Dark lines on a light surface, light ones on a dark one.
        let light = dot(in.color, vec3<f32>(0.2126, 0.7152, 0.0722));
        let ink = select(vec3<f32>(0.02), vec3<f32>(0.85), light < 0.12);
        lit = mix(lit, ink, line * 0.7);
    }
    return vec4<f32>(lit, 1.0);
}
