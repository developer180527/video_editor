// 0 threshold (%), 1 radius (px), 2 intensity (%), 3 colour: what is
// brighter than the threshold spreads light around it.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = src(i.uv);
    let thr = pow(params.values[0].x / 100.0, 2.4);
    let sigma = params.values[1].x * 0.5 * params.scale;
    let lod = max(log2(sigma / 2.0), 0.0);
    let d = vec2f(sigma * 0.75) / texels();
    var acc = vec3f(0.0);
    var wsum = 0.0;
    for (var y = -4; y <= 4; y++) {
        for (var x = -4; x <= 4; x++) {
            let k = vec2f(f32(x), f32(y));
            let w = exp(-0.28125 * dot(k, k));
            let t = tap(source, i.uv + k * d, lod, true);
            // What is brighter than the threshold, keeping its colour.
            let l = luma(t.rgb);
            acc += w * t.rgb * (max(l - thr * t.a, 0.0) / max(l, 1e-6));
            wsum += w;
        }
    }
    let glow = acc / wsum * params.values[2].x / 100.0 * params.values[3].rgb;
    return vec4f(p.rgb + glow, max(p.a, clamp(max(glow.r, max(glow.g, glow.b)), 0.0, 1.0)));
}
