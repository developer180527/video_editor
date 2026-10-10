// 0 amount (px at the frame's edge), 1 centre (%): red and blue land a
// little apart, more towards the edges, as through a simple lens.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = pct(params.values[1].xy);
    let off = (i.uv - c) * 2.0 * px_uv(params.values[0].x);
    // Clamped at the frame's edge, so no coloured fringe appears there.
    let r = src(i.uv - off);
    let g = src(i.uv);
    let b = src(i.uv + off);
    return vec4f(r.r, g.g, b.b, g.a);
}
