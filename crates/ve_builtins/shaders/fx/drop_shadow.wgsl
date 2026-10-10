// 0 colour, 1 opacity (%), 2 direction (degrees), 3 distance (px),
// 4 softness (px), 5 shadow only.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = src(i.uv);
    let off = heading(params.values[2].x) * px_uv(params.values[3].x);
    let a = blur2(source, i.uv - off, params.values[4].x * 0.5 * params.scale, true).a;
    let sa = a * params.values[1].x / 100.0 * params.values[0].a;
    let shadow = vec4f(params.values[0].rgb * sa, sa);
    if (on(5u)) {
        return shadow;
    }
    return p + shadow * (1.0 - p.a);
}
