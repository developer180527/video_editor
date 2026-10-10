// 0 angle (degrees), 1 radius (% of the frame's height), 2 centre (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = pct(params.values[2].xy);
    let q = to_square(i.uv, c);
    let big = max(params.values[1].x / 100.0, 1e-4);
    let t = max(1.0 - length(q) / big, 0.0);
    return src_clear(from_square(rot(q, radians(params.values[0].x) * t * t), c));
}
