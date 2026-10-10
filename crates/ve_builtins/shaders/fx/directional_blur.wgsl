// 0 direction (degrees, 0 = vertical), 1 blur length (px): a motion blur.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let d = heading(params.values[0].x) * px_uv(params.values[1].x);
    return smear(source, i.uv, d, true);
}
