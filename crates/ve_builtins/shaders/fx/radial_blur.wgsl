// 0 amount, 1 type (spin / zoom), 2 centre (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = pct(params.values[2].xy);
    let amount = params.values[0].x / 100.0;
    if (choice(1u) == 1) {
        // Zoom: along the line to the centre.
        let d = (i.uv - c) * amount * 0.5;
        return smear(source, i.uv - d * 0.5, d, true);
    }
    // Spin: along the circle around the centre.
    let q = to_square(i.uv, c);
    let span = amount * radians(30.0);
    let len = span * length(q) * texels().y;
    let n = i32(clamp(ceil(len / 1.5), 1.0, 64.0));
    if (n <= 1) {
        return src_clear(i.uv);
    }
    let lod = max(log2(len / f32(n) / 1.5), 0.0);
    var acc = vec4f(0.0);
    for (var k = 0; k < n; k++) {
        let f = f32(k) / f32(n - 1) - 0.5;
        acc += tap(source, from_square(rot(q, f * span), c), lod, true);
    }
    return acc / f32(n);
}
