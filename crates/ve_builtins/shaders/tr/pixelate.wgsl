// 0 largest block (px, at the middle): A breaks into blocks that turn
// into B.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = params.progress;
    let b = params.values[0].x * params.scale * sin(PI * p);
    let m = smoothstep(0.4, 0.6, p);
    if (b < 1.0) {
        return mix(src(i.uv), src_b(i.uv), m);
    }
    let uv = (floor(i.uv * texels() / b) + 0.5) * b / texels();
    let lod = log2(b);
    return mix(textureSampleLevel(source, source_sampler, uv, lod), textureSampleLevel(source_b, source_sampler, uv, lod), m);
}
