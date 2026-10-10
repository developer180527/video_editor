// 0 orientation (vertical / horizontal), 1 softness (%): B opens out
// from the middle.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let f = select(abs(i.uv.x - 0.5), abs(i.uv.y - 0.5), choice(0u) == 1) * 2.0;
    return mix(src(i.uv), src_b(i.uv), reveal(f, params.progress, params.values[1].x / 100.0));
}
