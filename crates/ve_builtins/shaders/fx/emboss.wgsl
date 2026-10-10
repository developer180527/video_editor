// 0 direction (degrees), 1 relief (px), 2 contrast (%), 3 blend with
// original (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let d = heading(params.values[0].x) * px_uv(max(params.values[1].x, 0.5));
    let e = luma(encode(straight(src(i.uv - d)).rgb)) - luma(encode(straight(src(i.uv + d)).rgb));
    let grey = vec3f(0.5 + e * params.values[2].x / 100.0);
    let o = mix(grey, encode(s.rgb), params.values[3].x / 100.0);
    return premul(vec4f(decode(max(o, vec3f(0.0))), s.a));
}
