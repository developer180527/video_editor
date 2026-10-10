// 0 amount (-100 darker .. 100 lighter), 1 midpoint, 2 roundness
// (-100 squarer .. 100 rounder), 3 feather.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let r = params.values[2].x / 100.0;
    var q = abs(i.uv - 0.5) * 2.0;
    q.x *= mix(1.0, aspect(), max(r, 0.0));
    let e = 2.0 + max(-r, 0.0) * 6.0;
    let d = pow(pow(q.x, e) + pow(q.y, e), 1.0 / e);
    let m = params.values[1].x / 100.0 * 1.4;
    let v = smoothstep(m, m + params.values[3].x / 100.0 + 1e-3, d);
    let amt = params.values[0].x / 100.0;
    let c = encode(s.rgb);
    let o = select(c + (vec3f(1.0) - c) * amt * v, c * (1.0 + amt * v), amt < 0.0);
    return premul(vec4f(decode(max(o, vec3f(0.0))), s.a));
}
