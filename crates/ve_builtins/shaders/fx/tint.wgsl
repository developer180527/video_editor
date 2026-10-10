// 0 map black to, 1 map white to, 2 amount (%): brightness becomes a
// blend between the two colours.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let c = encode(s.rgb);
    let t = mix(encode(params.values[0].rgb), encode(params.values[1].rgb), luma(c));
    let o = mix(c, t, params.values[2].x / 100.0);
    return premul(vec4f(decode(max(o, vec3f(0.0))), s.a));
}
