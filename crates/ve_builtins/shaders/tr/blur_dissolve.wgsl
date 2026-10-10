// 0 blur (px at the middle): both pictures blur as they cross.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = params.progress;
    let sigma = params.values[0].x * 0.5 * params.scale * sin(PI * p);
    let a = blur2(source, i.uv, sigma, false);
    let b = blur2(source_b, i.uv, sigma, false);
    return mix(a, b, smoothstep(0.0, 1.0, p));
}
