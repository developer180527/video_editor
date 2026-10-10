// 0 slats, 1 orientation (horizontal / vertical), 2 softness (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let n = max(round(params.values[0].x), 1.0);
    let f = fract(select(i.uv.y, i.uv.x, choice(1u) == 1) * n);
    return mix(src(i.uv), src_b(i.uv), reveal(f, params.progress, params.values[2].x / 100.0));
}
