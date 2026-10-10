// 0 direction (from left / right / top / bottom), 1 blur (%): a fast
// push, blurred along its motion.
fn pushed(uv: vec2f, d: vec2f, p: f32) -> vec4f {
    let ub = uv + d * (1.0 - p);
    if (inside(ub)) {
        return src_b(ub);
    }
    return src_clear(uv - d * p);
}

@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = smoothstep(0.0, 1.0, params.progress);
    let d = travel(0u);
    let len = sin(PI * params.progress) * params.values[1].x / 100.0 * 0.5;
    let n = i32(clamp(ceil(len * texels().x / 3.0), 1.0, 32.0));
    if (n <= 1) {
        return pushed(i.uv, d, p);
    }
    var acc = vec4f(0.0);
    for (var k = 0; k < n; k++) {
        acc += pushed(i.uv + d * len * (f32(k) / f32(n - 1) - 0.5), d, p);
    }
    return acc / f32(n);
}
