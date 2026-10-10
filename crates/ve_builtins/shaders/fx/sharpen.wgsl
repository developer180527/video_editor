// 0 amount (%): the picture minus its neighbours' average.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let t = 1.0 / texels();
    let p = src(i.uv);
    let n = (src(i.uv + vec2f(t.x, 0.0)) + src(i.uv - vec2f(t.x, 0.0)) + src(i.uv + vec2f(0.0, t.y)) + src(i.uv - vec2f(0.0, t.y))) * 0.25;
    let s = straight(p);
    let e = encode(s.rgb);
    let o = e + (e - encode(straight(n).rgb)) * params.values[0].x / 100.0;
    return premul(vec4f(decode(max(o, vec3f(0.0))), s.a));
}
