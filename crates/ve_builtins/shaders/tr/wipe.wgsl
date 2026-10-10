// 0 direction (degrees, 0 = left to right), 1 softness (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let a = radians(params.values[0].x);
    let d = vec2f(cos(a), sin(a));
    let q = vec2f((i.uv.x - 0.5) * aspect(), i.uv.y - 0.5);
    let e = 0.5 * (aspect() * abs(d.x) + abs(d.y));
    let f = (dot(q, d) + e) / (2.0 * e);
    return mix(src(i.uv), src_b(i.uv), reveal(f, params.progress, params.values[1].x / 100.0));
}
