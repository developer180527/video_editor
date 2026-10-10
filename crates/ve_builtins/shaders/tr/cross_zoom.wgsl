// 0 strength (%): A rushes towards the viewer and B settles out of it,
// blurred along the zoom.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = params.progress;
    let s = params.values[0].x / 100.0;
    let blur = sin(PI * p) * s * 0.4;
    let za = 1.0 + 2.0 * s * smoothstep(0.0, 1.0, p);
    let zb = 1.0 + 2.0 * s * smoothstep(0.0, 1.0, 1.0 - p);
    let m = smoothstep(0.35, 0.65, p);
    let c = vec2f(0.5);
    let d = i.uv - c;
    let len = length(d * texels()) * blur;
    let n = i32(clamp(ceil(len / 2.0), 1.0, 32.0));
    let lod = max(log2(len / f32(n) / 1.5), 0.0);
    var acc = vec4f(0.0);
    for (var k = 0; k < n; k++) {
        let f = 1.0 - blur * f32(k) / f32(max(n - 1, 1));
        let a = textureSampleLevel(source, source_sampler, c + d * f / za, lod);
        let b = textureSampleLevel(source_b, source_sampler, c + d * f / zb, lod);
        acc += mix(a, b, m);
    }
    return acc / f32(n);
}
