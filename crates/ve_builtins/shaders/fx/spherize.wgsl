// 0 amount (-100 pinch .. 100 bulge), 1 radius (% of the frame's
// height), 2 centre (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = pct(params.values[2].xy);
    let q = to_square(i.uv, c);
    let big = max(params.values[1].x / 100.0, 1e-4);
    let x = length(q) / big;
    if (x >= 1.0 || x <= 0.0) {
        return src_clear(i.uv);
    }
    let e = exp2(params.values[0].x / 100.0);
    return src_clear(from_square(q * (pow(x, e) / x), c));
}
