// B is added in over the first half; A fades out over the second.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = params.progress;
    let o = src(i.uv) * clamp(2.0 - 2.0 * p, 0.0, 1.0) + src_b(i.uv) * clamp(2.0 * p, 0.0, 1.0);
    return vec4f(o.rgb, min(o.a, 1.0));
}
