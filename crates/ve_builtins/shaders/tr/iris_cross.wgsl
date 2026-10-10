// 0 centre (%), 1 softness (%): B opens in a cross.
fn metric(q: vec2f) -> f32 {
    return min(abs(q.x), abs(q.y));
}

@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = pct(params.values[0].xy);
    let far = max(max(metric(to_square(vec2f(0.0, 0.0), c)), metric(to_square(vec2f(1.0, 0.0), c))),
                  max(metric(to_square(vec2f(0.0, 1.0), c)), metric(to_square(vec2f(1.0, 1.0), c))));
    let f = metric(to_square(i.uv, c)) / max(far, 1e-6);
    return mix(src(i.uv), src_b(i.uv), reveal(f, params.progress, params.values[1].x / 100.0));
}
