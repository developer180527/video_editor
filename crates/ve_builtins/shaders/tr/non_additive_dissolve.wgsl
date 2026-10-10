// Whichever of the two (each faded by progress) is brighter shows.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = params.progress;
    let a = src(i.uv) * (1.0 - p);
    let b = src_b(i.uv) * p;
    let la = luma(a.rgb);
    let lb = luma(b.rgb);
    return select(a, b, lb > la || (lb == la && p >= 0.5));
}
