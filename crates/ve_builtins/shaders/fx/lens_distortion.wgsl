// 0 curvature (-100 barrel .. 100 pincushion), 1 centre (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = pct(params.values[1].xy);
    let q = to_square(i.uv, c);
    let r = length(vec2f(aspect(), 1.0)) * 0.5;
    let r2 = dot(q, q) / (r * r);
    let k = -params.values[0].x / 100.0 * 0.5;
    return src_clear(from_square(q * (1.0 + k * r2), c));
}
