// 0..2 shadows R G B, 3..5 midtones, 6..8 highlights (-100..100),
// 9 preserve luminosity.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let v = params.values;
    var c = encode(s.rgb);
    let l0 = luma(c);
    let l = clamp(l0, 0.0, 1.0);
    let ws = 1.0 - smoothstep(0.0, 0.5, l);
    let wh = smoothstep(0.5, 1.0, l);
    let wm = 1.0 - ws - wh;
    let shadows = vec3f(v[0].x, v[1].x, v[2].x);
    let mids = vec3f(v[3].x, v[4].x, v[5].x);
    let highs = vec3f(v[6].x, v[7].x, v[8].x);
    c = c + (shadows * ws + mids * wm + highs * wh) * 0.002;
    if (v[9].x > 0.5) {
        c = c + vec3f(l0 - luma(c));
    }
    return premul(vec4f(decode(max(c, vec3f(0.0))), s.a));
}
