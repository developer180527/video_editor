// 0 blocks across, 1 softness (%): B arrives a block at a time.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let n = max(round(params.values[0].x), 1.0);
    let cell = floor(vec2f(i.uv.x * n, i.uv.y * n / aspect()));
    let f = hash12(cell + vec2f(7.0, 3.0));
    return mix(src(i.uv), src_b(i.uv), reveal(f, params.progress, params.values[1].x / 100.0));
}
