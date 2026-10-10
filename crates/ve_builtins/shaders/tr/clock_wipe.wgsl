// 0 direction (clockwise / counter-clockwise), 1 softness (%): a hand
// sweeps round from twelve o'clock.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let q = vec2f((i.uv.x - 0.5) * aspect(), i.uv.y - 0.5);
    var f = fract(atan2(q.x, -q.y) / TAU);
    if (choice(0u) == 1) {
        f = fract(1.0 - f);
    }
    return mix(src(i.uv), src_b(i.uv), reveal(f, params.progress, params.values[1].x / 100.0));
}
