// 0 brightness, 1 contrast (both -100..100), on display-encoded values.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let b = params.values[0].x / 200.0;
    // Contrast is a slope around mid grey: flat at -100, exactly 1 at 0,
    // 100 at 100.
    let ct = clamp(params.values[1].x / 100.0, -1.0, 1.0);
    let k = select(1.0 + ct, 1.0 / (1.0 - 0.99 * ct), ct > 0.0);
    let c = (encode(s.rgb) + b - 0.5) * k + 0.5;
    return premul(vec4f(decode(max(c, vec3f(0.0))), s.a));
}
