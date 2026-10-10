// 0 threshold (%), 1 softness (%), 2 invert (key out the bright parts).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = src(i.uv);
    let l = luma(encode(straight(p).rgb));
    let t = params.values[0].x / 100.0;
    var a = smoothstep(t, t + params.values[1].x / 100.0 + 1e-4, l);
    if (on(2u)) {
        a = 1.0 - a;
    }
    return p * a;
}
