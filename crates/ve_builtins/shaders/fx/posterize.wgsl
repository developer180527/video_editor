// 0 levels: brightness falls into that many steps, each colour keeping its
// hue. (Rounding each channel would shift hues in ACEScg, whose channels
// are not the monitor's; ABI v1 shaders are not told the working space.)
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let n = max(round(params.values[0].x), 2.0) - 1.0;
    let c = encode(max(s.rgb, vec3f(0.0)));
    let y = luma(c);
    let q = round(clamp(y, 0.0, 1.0) * n) / n;
    let o = select(vec3f(q), c * (q / y), y > 1e-4);
    return premul(vec4f(decode(o), s.a));
}
