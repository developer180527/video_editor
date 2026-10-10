// 0 corner (upper left / upper right / lower left / lower right),
// 1 softness (%): B grows out of a corner.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = choice(0u);
    let x = select(i.uv.x, 1.0 - i.uv.x, c == 1 || c == 3);
    let y = select(i.uv.y, 1.0 - i.uv.y, c >= 2);
    return mix(src(i.uv), src_b(i.uv), reveal(max(x, y), params.progress, params.values[1].x / 100.0));
}
