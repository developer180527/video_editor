// 0 bands, 1 orientation (horizontal / vertical), 2 softness (%): bands
// wipe in from alternate sides.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let n = max(round(params.values[0].x), 1.0);
    let vertical = choice(1u) == 1;
    let across = select(i.uv.y, i.uv.x, vertical);
    let along = select(i.uv.x, i.uv.y, vertical);
    let odd = (i32(floor(across * n)) & 1) == 1;
    let f = select(along, 1.0 - along, odd);
    return mix(src(i.uv), src_b(i.uv), reveal(f, params.progress, params.values[2].x / 100.0));
}
