// 0 softness (%), 1 invert: B shows through A's darkest parts first
// (brightest, inverted).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let a = src(i.uv);
    var f = clamp(luma(encode(straight(a).rgb)), 0.0, 1.0);
    if (on(1u)) {
        f = 1.0 - f;
    }
    return mix(a, src_b(i.uv), reveal(f, params.progress, params.values[0].x / 100.0));
}
