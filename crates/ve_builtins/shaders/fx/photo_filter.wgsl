// 0 filter colour, 1 density (%), 2 preserve luminosity: a coloured
// glass in front of the lens, in linear light.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    var f = s.rgb * params.values[0].rgb;
    if (on(2u)) {
        f = f * (luma(s.rgb) / max(luma(f), 1e-6));
    }
    return premul(vec4f(mix(s.rgb, f, params.values[1].x / 100.0), s.a));
}
