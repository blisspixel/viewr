// Draws one image as a generated quad. Opaque images use an sRGB texture, so
// sampling linearizes on read and the sRGB surface re-encodes on write.
// A texture with any alpha below 255 is already premultiplied linear RGBA8;
// that sample is returned unchanged and blended over the background.
// The checkerboard entry point paints screen-space cells under the same quad.

struct Placement {
    // xy: half-extent as a fraction of the viewport. zw: center offset in NDC.
    scale_offset: vec4<f32>,
    // 2x2 matrix for UV transformations (rotation and flipping).
    uv_matrix: vec4<f32>,
    // x0, y0, x1, y1 in UV space.
    crop_rect: vec4<f32>,
};

struct CheckerParams {
    color_a: vec4<f32>,
    color_b: vec4<f32>,
    cell_px: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> place: Placement;
@group(0) @binding(1) var image_tex: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;
@group(1) @binding(0) var<uniform> checker: CheckerParams;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    // Two triangles covering a unit quad, corners in [-1, 1].
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0,  1.0),
    );
    var order = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);

    let corner = corners[order[index]];
    let scale = place.scale_offset.xy;
    let offset = place.scale_offset.zw;

    var out: VsOut;
    out.clip = vec4<f32>(corner * scale + offset, 0.0, 1.0);

    // Map clip-space corner to texture space, flipping y so the image is upright.
    let base_uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    let centered = base_uv - vec2<f32>(0.5, 0.5);

    let m = mat2x2<f32>(place.uv_matrix.xy, place.uv_matrix.zw);
    out.uv = m * centered + vec2<f32>(0.5, 0.5);

    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var color = textureSample(image_tex, image_sampler, in.uv);
    if (place.crop_rect.z > place.crop_rect.x) {
        if (in.uv.x < place.crop_rect.x || in.uv.x > place.crop_rect.z ||
            in.uv.y < place.crop_rect.y || in.uv.y > place.crop_rect.w) {
            // ~50% dim outside the live crop (DESIGN: mode must be obvious).
            color = vec4<f32>(color.rgb * 0.45, color.a);
        }
    }
    return color;
}

@fragment
fn fs_checker(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // Framebuffer pixels, not image UVs, so pan and zoom do not move the cells.
    let cell = max(checker.cell_px, 1.0);
    let x = i32(floor(pos.x / cell));
    let y = i32(floor(pos.y / cell));
    if ((x + y) & 1) == 0 {
        return checker.color_a;
    }
    return checker.color_b;
}
